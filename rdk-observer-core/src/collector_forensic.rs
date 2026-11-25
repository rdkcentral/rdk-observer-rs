use crate::process_worker::ProcessId;
use anyhow::{anyhow, bail};
use aya::maps::{HashOfMaps, Map, MapData, MapError};
use aya::programs::trace_point::TracePointLinkId;
use aya::programs::{Program, TracePoint};
use aya::{Ebpf, include_bytes_aligned};
use parking_lot::RwLock;
use rdk_observer_ebpf_common::{EbpfThreadStats, MAX_THREADS_PER_PROCESS};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use tracing::error;

////////////////////////////////////////////////////////////////////////////////
const TRACEPOINTS: &[(&str, &str, &str)] = &[("on_sched_switch", "sched", "sched_switch")];

type ThreadStatsMap = aya::maps::HashMap<MapData, u32, EbpfThreadStats>;
type ProcessesMap = HashOfMaps<MapData, u32, ThreadStatsMap>;

////////////////////////////////////////////////////////////////////////////////
#[derive(Clone)]
pub struct ForensicCollector {
    shared: Arc<RwLock<Shared>>,
}

struct Shared {
    demand: HashMap<ProcessId, ProcessEntry>,
    ebpf: Ebpf,
    process_map: ProcessesMap,
    registrations: Option<Vec<(&'static str, TracePointLinkId)>>,
}

struct ProcessEntry {
    subscriptions: usize,
    threads: Option<Arc<ThreadStatsMap>>,
}

impl ForensicCollector {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn new() -> anyhow::Result<Self> {
        let ebpf_object = include_bytes_aligned!(concat!(env!("OUT_DIR"), "/rdk-observer-ebpf"));

        let mut ebpf = match Ebpf::load(ebpf_object) {
            Ok(e) => e,
            Err(e) => bail!("failed to load eBPF program: {e}"),
        };

        let process_map = take_map(&mut ebpf, "PROCESS_THREADS")?;

        let mut trace_point_loaded = false;
        for (name, _, _) in TRACEPOINTS {
            trace_point_loaded |= load_tracepoint(&mut ebpf, name);
        }

        if !trace_point_loaded {
            bail!("failed to load all trace points");
        }

        Ok(Self {
            shared: Arc::new(RwLock::new(Shared {
                demand: Default::default(),
                ebpf,
                process_map,
                registrations: None,
            })),
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn track_process(&self, process_id: ProcessId) -> ForensicSubscription {
        let shared = &mut *self.shared.write();

        shared.registrations.get_or_insert_with(|| {
            TRACEPOINTS
                .iter()
                .filter_map(|(program, category, name)| {
                    let link_id = attach_tracepoint(&mut shared.ebpf, program, category, name)?;
                    Some((*program, link_id))
                })
                .collect()
        });

        let process = shared.demand.entry(process_id).or_insert_with(|| {
            let threads = Self::register_threads_map(&mut shared.process_map, process_id);
            ProcessEntry {
                subscriptions: 0,
                threads,
            }
        });
        process.subscriptions += 1;

        ForensicSubscription {
            process_id,
            shared: self.shared.clone(),
            threads: process.threads.clone(),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn register_threads_map(
        processes: &mut ProcessesMap,
        process_id: ProcessId,
    ) -> Option<Arc<ThreadStatsMap>> {
        let pid = process_id.0.cast_unsigned();

        let threads = ThreadStatsMap::create(MAX_THREADS_PER_PROCESS as u32, 0)
            .inspect_err(|e| error!("failed to create threads map for {pid}: {e}"))
            .ok()?;

        processes
            .insert(pid, &threads, 0)
            .inspect_err(|e| error!("failed to record process ID {pid}: {e}"))
            .ok()?;

        Some(Arc::new(threads))
    }
}

////////////////////////////////////////////////////////////////////////////////
pub struct ForensicSubscription {
    process_id: ProcessId,
    shared: Arc<RwLock<Shared>>,
    threads: Option<Arc<ThreadStatsMap>>,
}

impl ForensicSubscription {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn threads(&self) -> Option<&ThreadStatsMap> {
        self.threads.as_deref()
    }
}

impl Drop for ForensicSubscription {
    ////////////////////////////////////////////////////////////////////////////////
    fn drop(&mut self) {
        let shared = &mut *self.shared.write();
        let Entry::Occupied(mut entry) = shared.demand.entry(self.process_id) else {
            return;
        };

        let process = entry.get_mut();
        process.subscriptions = process.subscriptions.saturating_sub(1);
        if process.subscriptions > 0 {
            return;
        }

        // remove from the outer map first, inner map is freed once the last Arc is dropped
        if entry.remove().threads.is_some() {
            let process_id = self.process_id.0.cast_unsigned();
            if let Err(e) = shared.process_map.remove(&process_id) {
                error!("failed to remove process ID: {e}");
            }
        }

        if shared.demand.is_empty()
            && let Some(registration) = shared.registrations.take()
        {
            for (program, link_id) in registration {
                deattach_tracepoint(&mut shared.ebpf, program, link_id);
            }
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
fn find_tracepoint<'a>(ebpf: &'a mut Ebpf, program: &str) -> Option<&'a mut TracePoint> {
    let Some(program_mut) = ebpf.program_mut(program) else {
        error!("eBPF program {program} not found");
        return None;
    };
    let Program::TracePoint(tracepoint) = program_mut else {
        error!("eBPF program {program} is not a tracepoint: {program_mut:?}");
        return None;
    };
    Some(tracepoint)
}

////////////////////////////////////////////////////////////////////////////////
fn load_tracepoint(ebpf: &mut Ebpf, program: &str) -> bool {
    let Some(tracepoint) = find_tracepoint(ebpf, program) else {
        return false;
    };
    tracepoint
        .load()
        .inspect_err(|e| error!("failed to load tracepoint {program}: {e:?}"))
        .is_ok()
}

////////////////////////////////////////////////////////////////////////////////
fn attach_tracepoint(
    ebpf: &mut Ebpf,
    program: &str,
    category: &str,
    name: &str,
) -> Option<TracePointLinkId> {
    let tracepoint = find_tracepoint(ebpf, program)?;

    tracepoint
        .attach(category, name)
        .inspect_err(|e| error!("failed tp attach tracepoint {program} to {category}/{name}: {e}"))
        .ok()
}

////////////////////////////////////////////////////////////////////////////////
fn deattach_tracepoint(ebpf: &mut Ebpf, program: &str, link_id: TracePointLinkId) {
    let Some(tracepoint) = find_tracepoint(ebpf, program) else {
        return;
    };
    if let Err(e) = tracepoint.detach(link_id) {
        error!("failed tp deattach tracepoint {program}: {e}")
    }
}

////////////////////////////////////////////////////////////////////////////////
fn take_map<T: TryFrom<Map, Error = MapError>>(ebpf: &mut Ebpf, name: &str) -> anyhow::Result<T> {
    let Some(map) = ebpf.take_map(name) else {
        bail!("eBPF map {name} not found");
    };
    map.try_into()
        .map_err(|e| anyhow!("eBPF map {name} error: {e}"))
}
