//! Collection worker for process targets and system totals.
//!
//! This module will run collection tasks for the configured resources and scope.
//! Process collection will use identities from `process_registry` to read one
//! selected PID or collect a full process snapshot by sampling the registered
//! list. System collection will read device-wide totals independently of process
//! selection, following ADR-0001.
//!
//! Tasks will receive start and stop instructions from `process_worker` for
//! registered targets and report completion, target disappearance, and failures
//! back to it. `runtime` will manage system collection and the overall worker
//! lifecycle.
//! Tasks will call the applicable `collector_*` functions and combine readings
//! into timestamped observations, preserving raw values, units, process
//! identities, and unavailable measurements.
//! When configured, tasks will derive CPU utilization and I/O rates from
//! consecutive valid samples with elapsed time between them. A first sample
//! establishes a baseline; counter resets, missing targets, or identity changes
//! invalidate affected deltas.
//!
//! Per-process tasks will use adaptive intervals and bounded jitter, following
//! ADR-0002. Periodic and event-triggered reads will share a concurrency limit.
//! Full-list requests will be coordinated with active tasks to avoid duplicate
//! reads. Delayed work will not cause bursts of catch-up samples.
//!
//! Tasks will wait for output readiness before producing observations, following
//! the startup order managed by `runtime`. Observations will be sent directly
//! to `output_worker` through a channel.
//! Full-list collection will use bounded concurrency and will not produce an
//! atomic snapshot of all processes or the whole system. Collection and channel
//! failures will be reported to the coordinator or runtime; policies remain
//! to be defined.

use crate::output_model::{
    BlockDeviceIdentity, CpuCounters, CpuUtilization, IoRates, ObservationRecord, ProcessIdentity,
    ResourceRecord, StorageCounters, SystemCpuCounters, SystemRecord, SystemResourceRecord,
    SystemStorageCounters,
};
use crate::process_registry::{ProcessKey, ProcessRegistryEvent, RegisteredProcess};
use crate::process_worker::ProcessId;
use crate::runtime::RuntimeContext;
use rustix::time::{ClockId, clock_gettime};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::select;
use tokio::sync::Semaphore;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::{AbortHandle, JoinSet, spawn_blocking};
use tokio::time::{Instant, MissedTickBehavior};
use tracing::{error, info, instrument, trace, warn};

/// Safeguard to limit number of processes that can be collected at the same time
static PROCESS_COLLECTION_CONCURRENCY: Semaphore = Semaphore::const_new(4);

#[derive(Debug)]
pub enum CollectorEvent {
    System(SystemRecord),
    Process((ProcessId, ObservationRecord)),
}

/// Worker that collect system/process stats with some interval
pub struct CollectorWorker {
    /// current runtime context
    context: RuntimeContext,
    /// events produced to outside consumer
    output_events_tx: tokio::sync::mpsc::Sender<CollectorEvent>,
    /// per_pid workers
    per_pid_workers: HashMap<ProcessKey, AbortHandle>,
    /// collector tasks join set
    join_set: JoinSet<()>,
}

impl CollectorWorker {
    ////////////////////////////////////////////////////////////////////////////////
    #[instrument(skip_all, "collector")]
    pub async fn run(
        context: RuntimeContext,
        events_tx: tokio::sync::mpsc::Sender<CollectorEvent>,
    ) {
        let (snapshot, mut registry_events_rx) = context.registry.subscribe_with_snapshot();

        let mut this = Self {
            context,
            output_events_tx: events_tx.clone(),
            per_pid_workers: Default::default(),
            join_set: Default::default(),
        };

        this.join_set
            .spawn(SystemWorker::run(this.context.clone(), events_tx.clone()));
        this.sync_processes(snapshot);

        loop {
            select! {
                event = registry_events_rx.recv() => match event {
                    Ok(e) => this.handle_registry_event(e),
                    Err(RecvError::Lagged(count)) => {
                        warn!("missed {count} registry events, resyncing");
                        let (snapshot, rx) = this.context.registry.subscribe_with_snapshot();
                        registry_events_rx = rx;
                        this.sync_processes(snapshot);
                    }
                    Err(RecvError::Closed) => {
                        return info!("registry closed, stopping the worker");
                    }
                },
                _ = events_tx.closed() => {
                    return info!("events tx channel closed, stopping the worker");
                },
                Some(result) = this.join_set.join_next() => {
                    if let Err(e) = result && !e.is_cancelled() {
                        error!("task failed: {e}");
                    }
                }
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn sync_processes(&mut self, snapshot: Vec<RegisteredProcess>) {
        let registered: HashSet<_> = snapshot.into_iter().map(|e| e.key()).collect();

        for (_, task) in self
            .per_pid_workers
            .extract_if(|e, _| !registered.contains(e))
        {
            task.abort();
        }

        for process in registered {
            if !self.per_pid_workers.contains_key(&process) {
                self.handle_process_started(process);
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn handle_registry_event(&mut self, event: ProcessRegistryEvent) {
        match event {
            ProcessRegistryEvent::Started(e) => self.handle_process_started(e),
            ProcessRegistryEvent::Finished(e) => self.handle_process_finished(e),
            ProcessRegistryEvent::Updated(_) => {}
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn handle_process_started(&mut self, process: ProcessKey) {
        trace!("started: {process:?}");

        let handle = self.join_set.spawn(PerPidWorker::run(
            self.context.clone(),
            process,
            self.output_events_tx.clone(),
        ));

        if let Some(task) = self.per_pid_workers.insert(process, handle) {
            task.abort();
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn handle_process_finished(&mut self, process: ProcessKey) {
        trace!("finished: {process:?}");

        if let Some(task) = self.per_pid_workers.remove(&process) {
            task.abort();
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
struct SystemWorker {
    context: RuntimeContext,
    events_tx: tokio::sync::mpsc::Sender<CollectorEvent>,
    last_cpu_record: Option<(Duration, SystemCpuCounters)>,
    last_io_records: Option<(
        Duration,
        HashMap<BlockDeviceIdentity, SystemStorageCounters>,
    )>,
}

impl SystemWorker {
    ////////////////////////////////////////////////////////////////////////////////
    #[instrument(skip_all, "collector/system")]
    async fn run(context: RuntimeContext, events_tx: tokio::sync::mpsc::Sender<CollectorEvent>) {
        trace!("started");

        let mut interval = tokio::time::interval(context.config.system_collection_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut this = Self {
            context,
            events_tx,
            last_cpu_record: None,
            last_io_records: None,
        };

        loop {
            interval.tick().await;

            if this.events_tx.is_closed() {
                break;
            }

            if let Err(e) = this.collect_io().await {
                error!("io collector failed: {e}");
            }
            if let Err(e) = this.collect_cpu().await {
                error!("cpu collector failed: {e}");
            }
            if let Err(e) = this.collect_memory().await {
                error!("memory collector failed: {e}");
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn collect_io(&mut self) -> anyhow::Result<()> {
        trace!("collecting io");

        let context = self.context.clone();
        let mut records = spawn_blocking(move || context.components.read_system_io()).await??;

        let ts_monotonic = get_monotonic_time();
        let ts_unix_epoch = get_unix_epoch_time();

        fn to_bps(new: Option<u64>, old: Option<u64>, interval: Duration) -> Option<f64> {
            if interval.is_zero() {
                return None;
            }
            // new and old are measured in sectors, which are always 512 bytes each
            let diff = new?.checked_sub(old?)? * 512;
            Some(diff as f64 / interval.as_secs_f64())
        }

        for record in records.iter_mut() {
            let Some((last_ts, last_records)) = self.last_io_records.as_ref() else {
                continue;
            };
            let Some(last_record) = last_records.get(&record.device_identity) else {
                continue;
            };

            let interval = ts_monotonic.saturating_sub(*last_ts);
            let read_bps = to_bps(
                record.counters.read_sectors,
                last_record.read_sectors,
                interval,
            );
            let write_bps = to_bps(
                record.counters.write_sectors,
                last_record.write_sectors,
                interval,
            );

            if read_bps.is_some() || write_bps.is_some() {
                record.rates = Some(IoRates {
                    interval_ns: interval.as_nanos() as u64,
                    read_bytes_per_second: read_bps,
                    write_bytes_per_second: write_bps,
                });
            }
        }

        self.last_io_records = Some((
            ts_monotonic,
            records
                .iter()
                .map(|e| (e.device_identity.clone(), e.counters.clone()))
                .collect(),
        ));

        self.events_tx
            .send(CollectorEvent::System(SystemRecord {
                timestamp_unix_ns: ts_unix_epoch.as_nanos() as u64,
                timestamp_monotonic_ns: ts_monotonic.as_nanos() as u64,
                resource: SystemResourceRecord::Storage(records),
            }))
            .await?;

        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn collect_cpu(&mut self) -> anyhow::Result<()> {
        trace!("collecting cpu");

        let context = self.context.clone();
        let mut record = spawn_blocking(move || context.components.read_system_cpu()).await??;

        let ts_monotonic = get_monotonic_time();
        let ts_unix_epoch = get_unix_epoch_time();

        fn busy_ticks(c: &SystemCpuCounters) -> u64 {
            c.user_ticks.unwrap_or(0)
                + c.nice_ticks.unwrap_or(0)
                + c.system_ticks.unwrap_or(0)
                + c.irq_ticks.unwrap_or(0)
                + c.softirq_ticks.unwrap_or(0)
                + c.steal_ticks.unwrap_or(0)
        }

        fn idle_ticks(c: &SystemCpuCounters) -> u64 {
            c.idle_ticks.unwrap_or(0) + c.iowait_ticks.unwrap_or(0)
        }

        if let Some((last_ts, last_counters)) = self.last_cpu_record.as_ref() {
            let interval = ts_monotonic.saturating_sub(*last_ts);

            // busy always increases, idle can slightly decrease because of iowait
            let busy = busy_ticks(&record.counters).checked_sub(busy_ticks(last_counters));
            let idle = idle_ticks(&record.counters).saturating_sub(idle_ticks(last_counters));

            if let Some(busy) = busy
                && busy + idle > 0
            {
                record.utilization = Some(CpuUtilization {
                    interval_ns: interval.as_nanos() as u64,
                    percent: busy as f64 / (busy + idle) as f64 * 100.0,
                });
            }
        }

        self.last_cpu_record = Some((ts_monotonic, record.counters.clone()));
        self.events_tx
            .send(CollectorEvent::System(SystemRecord {
                timestamp_unix_ns: ts_unix_epoch.as_nanos() as u64,
                timestamp_monotonic_ns: ts_monotonic.as_nanos() as u64,
                resource: SystemResourceRecord::Cpu(record),
            }))
            .await?;

        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn collect_memory(&mut self) -> anyhow::Result<()> {
        trace!("collecting memory");

        let context = self.context.clone();
        let record = spawn_blocking(move || context.components.read_system_memory()).await??;

        let ts_monotonic = get_monotonic_time();
        let ts_unix_epoch = get_unix_epoch_time();

        self.events_tx
            .send(CollectorEvent::System(SystemRecord {
                timestamp_unix_ns: ts_unix_epoch.as_nanos() as u64,
                timestamp_monotonic_ns: ts_monotonic.as_nanos() as u64,
                resource: SystemResourceRecord::Memory(record),
            }))
            .await?;

        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////
struct PerPidWorker {
    context: RuntimeContext,
    pending_events: Vec<CollectorEvent>,
    process: ProcessKey,
    last_cpu_record: Option<(Duration, CpuCounters)>,
    last_storage_record: Option<(Duration, StorageCounters)>,
}

impl PerPidWorker {
    ////////////////////////////////////////////////////////////////////////////////
    #[instrument(skip_all, "collector/process", fields(pid = %process.pid))]
    async fn run(
        context: RuntimeContext,
        process: ProcessKey,
        events_tx: tokio::sync::mpsc::Sender<CollectorEvent>,
    ) {
        trace!("started");

        let mut frame_index = 0u32;
        let mut interval =
            tokio::time::interval(context.config.process_collection_interval_initial);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut this = Self {
            context,
            pending_events: vec![],
            process,
            last_cpu_record: None,
            last_storage_record: None,
        };

        loop {
            interval.tick().await;

            if let Ok(_permit) = PROCESS_COLLECTION_CONCURRENCY.acquire().await {
                // cpu validates process start time
                // if it fails - no need to collect other stats
                if let Err(e) = this.collect_cpu().await {
                    error!("cpu collector failed: {e}");
                } else {
                    if let Err(e) = this.collect_io().await {
                        error!("io collector failed: {e}");
                    }
                    if let Err(e) = this.collect_memory().await {
                        error!("memory collector failed: {e}");
                    }
                }
            }

            this.update_registry_from_events();

            for event in this.pending_events.drain(..) {
                if events_tx.send(event).await.is_err() {
                    return warn!("events_tx is closed, stopping");
                }
            }

            // first few frames are collected with fast interval
            // all consecutive frames are collected with slower interval
            frame_index = frame_index.saturating_add(1);
            if frame_index == 2 {
                let jitter_range = this.context.config.process_collection_interval_ongoing;
                let jitter_in = rand::random::<f32>() * jitter_range.as_secs_f32();
                let jitter_at = Instant::now() + Duration::from_secs_f32(jitter_in);

                interval = tokio::time::interval_at(jitter_at, jitter_range);
                interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Stores the latest records of this frame in the registry with a single lock
    fn update_registry_from_events(&self) {
        let (mut io, mut cpu, mut memory) = (None, None, None);

        for event in &self.pending_events {
            let CollectorEvent::Process((_, record)) = event else {
                continue;
            };

            let ts = Duration::from_nanos(record.timestamp_unix_ns);
            match &record.resource {
                ResourceRecord::Storage(e) => io = Some((ts, e.clone())),
                ResourceRecord::Cpu(e) => cpu = Some((ts, e.clone())),
                ResourceRecord::Memory(e) => memory = Some((ts, e.clone())),
                ResourceRecord::Threads(_) => {
                    // registry doesn't keep thread records yet
                }
            }
        }

        if io.is_none() && cpu.is_none() && memory.is_none() {
            return;
        }

        // 1. all records are cloned outside the lock to reduce locked time
        // 2. records that failed this frame keep their previous value
        self.context.registry.update_process(&self.process, |e| {
            if let Some(io) = io {
                e.last_io_record = Some(io);
            }
            if let Some(cpu) = cpu {
                e.last_cpu_record = Some(cpu);
            }
            if let Some(memory) = memory {
                e.last_memory_record = Some(memory);
            }
        });
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn collect_io(&mut self) -> anyhow::Result<()> {
        trace!("collecting io");

        let process = self.process.pid;
        let context = self.context.clone();
        let mut record =
            spawn_blocking(move || context.components.read_process_io(process)).await??;

        let ts_monotonic = get_monotonic_time();
        let ts_unix_epoch = get_unix_epoch_time();

        fn to_bps(new: Option<u64>, old: Option<u64>, interval: Duration) -> Option<f64> {
            if interval.is_zero() {
                return None;
            }
            let diff = new?.checked_sub(old?)?;
            Some(diff as f64 / interval.as_secs_f64())
        }

        if let Some((last_ts, last_counters)) = self.last_storage_record.as_ref() {
            let interval = ts_monotonic.saturating_sub(*last_ts);
            let read_bps = to_bps(
                record.counters.read_bytes,
                last_counters.read_bytes,
                interval,
            );
            let write_bps = to_bps(
                record.counters.write_bytes,
                last_counters.write_bytes,
                interval,
            );

            if read_bps.is_some() || write_bps.is_some() {
                record.rates = Some(IoRates {
                    interval_ns: interval.as_nanos() as u64,
                    read_bytes_per_second: read_bps,
                    write_bytes_per_second: write_bps,
                });
            }
        }

        self.last_storage_record = Some((ts_monotonic, record.counters.clone()));
        self.schedule_record(ObservationRecord {
            process_identity: self.process_identity(),
            timestamp_unix_ns: ts_unix_epoch.as_nanos() as u64,
            timestamp_monotonic_ns: ts_monotonic.as_nanos() as u64,
            resource: ResourceRecord::Storage(record),
        });
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn collect_cpu(&mut self) -> anyhow::Result<()> {
        trace!("collecting cpu");

        let process = self.process;
        let context = self.context.clone();
        let mut record =
            spawn_blocking(move || context.components.read_process_cpu(process)).await??;

        let ts_monotonic = get_monotonic_time();
        let ts_unix_epoch = get_unix_epoch_time();

        fn busy_ticks(c: &CpuCounters) -> u64 {
            c.user_ticks.unwrap_or(0) + c.system_ticks.unwrap_or(0)
        }

        if let Some((last_ts, last_counters)) = self.last_cpu_record.as_ref() {
            let interval = ts_monotonic.saturating_sub(*last_ts);
            let busy = busy_ticks(&record.counters).checked_sub(busy_ticks(last_counters));

            if let Some(busy) = busy
                && record.clock_ticks_per_second > 0
                && !interval.is_zero()
            {
                let busy_secs = busy as f64 / record.clock_ticks_per_second as f64;
                record.utilization = Some(CpuUtilization {
                    interval_ns: interval.as_nanos() as u64,
                    percent: busy_secs / interval.as_secs_f64() * 100.0,
                });
            }
        }

        self.last_cpu_record = Some((ts_monotonic, record.counters.clone()));
        self.schedule_record(ObservationRecord {
            process_identity: self.process_identity(),
            timestamp_unix_ns: ts_unix_epoch.as_nanos() as u64,
            timestamp_monotonic_ns: ts_monotonic.as_nanos() as u64,
            resource: ResourceRecord::Cpu(record),
        });
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn collect_memory(&mut self) -> anyhow::Result<()> {
        trace!("collecting memory");

        let process = self.process.pid;
        let context = self.context.clone();
        let record =
            spawn_blocking(move || context.components.read_process_memory(process)).await??;

        let ts_monotonic = get_monotonic_time();
        let ts_unix_epoch = get_unix_epoch_time();

        self.schedule_record(ObservationRecord {
            process_identity: self.process_identity(),
            timestamp_unix_ns: ts_unix_epoch.as_nanos() as u64,
            timestamp_monotonic_ns: ts_monotonic.as_nanos() as u64,
            resource: ResourceRecord::Memory(record),
        });
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn process_identity(&self) -> ProcessIdentity {
        ProcessIdentity {
            pid: self.process.pid.0.cast_unsigned(),
            start_time_ticks: self.process.start_time,
            logical_label: None,
            security_label: None,
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn schedule_record(&mut self, record: ObservationRecord) {
        self.pending_events
            .push(CollectorEvent::Process((self.process.pid, record)));
    }
}

////////////////////////////////////////////////////////////////////////////////
fn get_unix_epoch_time() -> Duration {
    let spec = clock_gettime(ClockId::Realtime);
    Duration::new(spec.tv_sec as u64, spec.tv_nsec as u32)
}

////////////////////////////////////////////////////////////////////////////////
fn get_monotonic_time() -> Duration {
    let spec = clock_gettime(ClockId::Monotonic);
    Duration::new(spec.tv_sec as u64, spec.tv_nsec as u32)
}
