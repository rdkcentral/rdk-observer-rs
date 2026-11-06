//! Process target coordination worker.
//!
//! This module will run a coordinator started by `runtime` to receive discovered
//! processes and lifecycle events from `process_discovery`. It will apply the
//! configured selection rules and maintain targets in `process_registry`, using
//! PID and start time to identify each process.
//!
//! Scan results and netlink events will be reconciled without duplicating
//! targets or undoing newer lifecycle changes. Task IDs and thread-group IDs
//! must be distinguished: a thread exit is not proof of whole-process exit.
//! Validate ambiguous exits and delayed events before removing an identity,
//! preserving a live process that reused a PID. Partial scans must not evict
//! targets merely because identity information could not be read.
//!
//! The coordinator will start and stop per-process `collector_worker` tasks as
//! targets enter or leave the registry. It will coalesce prompt collection
//! requests on fork or exec and request collection across the registered list
//! when a full process snapshot is selected. Collection modes will share the
//! configured concurrency limit and avoid duplicate work for a target.
//!
//! Normal coordination will continue during forensic sessions. Anomaly triggers
//! routed to `runtime` will identify the affected process or thread for targeted
//! eBPF capture; routine lifecycle events and adaptive sampling changes alone
//! will not activate forensic mode. Target exit will also end its forensic
//! session, following ADR-0003.
//!
//! Collection tasks will report completion, target disappearance, and failures
//! to this coordinator so it can reconcile collection state with the registry.
//! Process lifecycle events will stay within discovery and collection
//! coordination. Resource observations will be collected by `collector_*` and
//! sent directly to `output_worker` for Protobuf preparation and storage.
//! Discovery and coordination will continue while output initializes.
//! Coordination failures will be reported to the runtime; the handling policy
//! remains to be defined.

use crate::process_discovery::{
    NetlinkProcessCommEvent, NetlinkProcessDiscovery, NetlinkProcessEvent, NetlinkProcessExecEvent,
    NetlinkProcessExitEvent, NetlinkProcessForkEvent,
};
use crate::process_registry::ProcessRegistryWriter;
use procfs::ProcError;
use procfs::process::{Process, Stat, StatFlags};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::time::{Duration, Instant};
use tokio::select;
use tokio::sync::watch::error::RecvError;
use tokio::task::{JoinError, JoinSet};
use tokio::time::MissedTickBehavior;
use tracing::{error, instrument, warn};

/// Interval at which whole process list is refreshed
const PID_REFETCH_INTERVAL: Duration = Duration::from_secs(30);

/// Same as [PID_REFETCH_INTERVAL] but for cases when connector socket
/// connection fails for any reason and interval might be different
const PID_REFETCH_INTERVAL_FALLBACK: Duration = PID_REFETCH_INTERVAL;

/// Dispatch unverified processes if exit event was not received during this time
const UNVERIFIED_PROCESS_TIMEOUT: Duration = Duration::from_secs(10);

/// Process command name used for unverified processes
/// when /proc info could not be read (usually short-lived)
const UNVERIFIED_PROCESS_COMMAND: &str = "(unverified)";

/// Identities read by a reconciliation scan, see [scan_processes]
type ScanResult = anyhow::Result<HashMap<ProcessId, Option<VerifiedProcessInfo>>>;

/// Typed Process ID
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct ProcessId(pub i32);

impl Display for ProcessId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, f)
    }
}

/// Typed Thread ID
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ThreadId {
    pub pid: i32,
    pub tid: i32,
}

impl Display for ThreadId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.tid, f)
    }
}

/// Resolved process info
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProcessInfo {
    pub pid: ProcessId,
    /// command name, might be after creation (fork -> comm)
    pub command: String,
    /// kernel thread (PF_KTHREAD), always `false` for unverified process
    pub kernel_thread: bool,
    /// start time in ticks from boot (estimate for unverified process),
    pub start_time: u64,
    /// exit_code when `Ok`, exit_signal when `Err`
    pub exit_result: Option<Result<u32, u32>>,
}

#[derive(Debug, Clone)]
struct VerifiedProcessInfo {
    info: ProcessInfo,
    /// all process threads, empty set means data is not available
    threads: HashSet<ThreadId>,
}

#[derive(Debug)]
struct UnverifiedProcessInfo {
    info: ProcessInfo,
    created_at: Instant,
}

pub struct ProcessWorker {
    /// list of currently active processes
    processes: HashMap<ProcessId, VerifiedProcessInfo>,
    /// list of unverified processes with /proc record missing (usually short-lived)
    unverified_processes: HashMap<ProcessId, UnverifiedProcessInfo>,
    /// real-time process discovery
    discovery: NetlinkProcessDiscovery,
    /// interval at which process list is fully updated
    refresh_interval: tokio::time::Interval,
    /// background refresh scan, at most one at a time
    scans: JoinSet<ScanResult>,
    /// scan was requested and waiting for execution
    scan_requested: bool,
    /// processes updated by events while a scan is running,
    /// their state is newer than what the scan has read
    scan_touched: HashSet<ProcessId>,
    /// processes finished before their main thread exit event was handled,
    /// so a delayed event is not reported again as an unverified process
    finished_processes: HashMap<ProcessId, Instant>,
    /// shared registry of verified processes, updated only by this worker
    registry: ProcessRegistryWriter,
}

impl ProcessWorker {
    ////////////////////////////////////////////////////////////////////////////////
    /// Runs until cancelled, registry is expected to be empty on start
    #[instrument(skip_all, "proc_worker")]
    pub async fn run(registry: ProcessRegistryWriter) {
        let mut refresh_interval = tokio::time::interval(PID_REFETCH_INTERVAL_FALLBACK);
        refresh_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut unverified_interval = tokio::time::interval(UNVERIFIED_PROCESS_TIMEOUT);
        unverified_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut this = Self {
            processes: Default::default(),
            unverified_processes: Default::default(),
            discovery: Default::default(),
            refresh_interval,
            scans: Default::default(),
            scan_requested: false,
            scan_touched: Default::default(),
            finished_processes: Default::default(),
            registry,
        };

        loop {
            select! {
                result = this.discovery.connected.changed() => {
                    this.handle_discovery_connected_changed(result).await;
                }
                event = this.discovery.events.recv() => {
                    this.handle_discovery_event(event).await;
                }
                _ = this.discovery.refresh.notified() => {
                    this.scan_requested = true;
                }
                _ = this.refresh_interval.tick() => {
                    this.scan_requested = true;
                }
                _ = unverified_interval.tick() => {
                    this.handle_unverified_processes_timeout();
                }
                Some(result) = this.scans.join_next() => {
                    this.handle_scan_finished(result);
                }
            }

            if this.scan_requested && this.scans.is_empty() {
                this.scan_requested = false;
                this.scans.spawn_blocking(scan_processes);
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_discovery_connected_changed(&mut self, result: Result<(), RecvError>) {
        if result.is_err() {
            self.discovery = NetlinkProcessDiscovery::default();
            return warn!("process discovery restarted");
        }

        let interval = match *self.discovery.connected.borrow_and_update() {
            true => PID_REFETCH_INTERVAL,
            false => PID_REFETCH_INTERVAL_FALLBACK,
        };

        self.refresh_interval = tokio::time::interval(interval);
        self.refresh_interval
            .set_missed_tick_behavior(MissedTickBehavior::Delay);
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_discovery_event(&mut self, event: Option<NetlinkProcessEvent>) {
        let Some(event) = event else {
            self.discovery = NetlinkProcessDiscovery::default();
            return warn!("process discovery restarted");
        };

        match event {
            NetlinkProcessEvent::Fork(e) if e.child_pid == e.child_tgid => {
                self.handle_discovery_event_fork_process(e).await;
            }
            NetlinkProcessEvent::Fork(e) => {
                self.handle_discovery_event_fork_thread(e);
            }
            NetlinkProcessEvent::Exec(e) => {
                self.handle_discovery_event_exec(e).await;
            }
            NetlinkProcessEvent::Comm(e) => {
                self.handle_discovery_event_comm(e).await;
            }
            NetlinkProcessEvent::Exit(e) => {
                self.handle_discovery_event_exit(e).await;
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_discovery_event_fork_process(&mut self, event: NetlinkProcessForkEvent) {
        let process_id = ProcessId(event.child_tgid);

        if !self.scans.is_empty() {
            self.scan_touched.insert(process_id);
        }

        if self.update_process_by_pid(process_id).await {
            return;
        }

        if let Some(old) = self.unverified_processes.insert(
            process_id,
            UnverifiedProcessInfo {
                info: ProcessInfo {
                    pid: process_id,
                    kernel_thread: false,
                    command: UNVERIFIED_PROCESS_COMMAND.to_string(),
                    start_time: event.ts_boot_ticks,
                    exit_result: None,
                },
                created_at: Instant::now(),
            },
        ) {
            self.registry.register_unverified_process(old.info);
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn handle_discovery_event_fork_thread(&mut self, event: NetlinkProcessForkEvent) {
        let process_id = ProcessId(event.child_tgid);

        if let Some(process) = self.processes.get_mut(&process_id)
            && event.ts_boot_ticks >= process.info.start_time
        {
            if !self.scans.is_empty() {
                self.scan_touched.insert(process_id);
            }
            process.threads.insert(ThreadId {
                pid: event.child_tgid,
                tid: event.child_pid,
            });
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_discovery_event_exec(&mut self, event: NetlinkProcessExecEvent) {
        let process_id = ProcessId(event.process_tgid);
        let tracked = self.processes.contains_key(&process_id);

        if !self.scans.is_empty() {
            self.scan_touched.insert(process_id);
        }

        if self.update_process_by_pid(process_id).await || tracked {
            return;
        }

        self.unverified_processes
            .entry(process_id)
            .or_insert_with(|| UnverifiedProcessInfo {
                info: ProcessInfo {
                    pid: process_id,
                    kernel_thread: false,
                    command: UNVERIFIED_PROCESS_COMMAND.to_string(),
                    start_time: event.ts_boot_ticks,
                    exit_result: None,
                },
                created_at: Instant::now(),
            });
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_discovery_event_comm(&mut self, event: NetlinkProcessCommEvent) {
        // 1.
        // only main thread rename changes process name,
        // other threads can be renamed via pthread_setname_np

        // 2.
        // queued event may predate state already refreshed by a scan,
        // so its command is only a hint and the current identity is re-read

        if event.process_pid != event.process_tgid {
            return;
        }

        let process_id = ProcessId(event.process_tgid);
        if !self.scans.is_empty() {
            self.scan_touched.insert(process_id);
        }

        if self.update_process_by_pid(process_id).await {
            return;
        }

        if let Some(process) = self.unverified_processes.get_mut(&process_id) {
            process.info.command = event.command;
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_discovery_event_exit(&mut self, event: NetlinkProcessExitEvent) {
        // 1.
        // pid equals to thread group id only means that main thread has stopped.
        // other thread can still be alive, so we need to wait until all of them are stopped

        // 2.
        // exit event of a previous pid owner may arrive after a new process with the same pid
        // appears, so it is accepted only when the exit happened after the process started.

        let process_id = ProcessId(event.process_tgid);
        let is_main_thread = event.process_pid == event.process_tgid;

        if !self.scans.is_empty() {
            self.scan_touched.insert(process_id);
        }

        // exit of an unknown thread group suppresses its entry of a running scan,
        // so its info is updated now to not delay discovery of a still alive process
        let tracked = self.processes.contains_key(&process_id);
        if !tracked && !self.unverified_processes.contains_key(&process_id) {
            let found = self.update_process_by_pid(process_id).await;

            // process exited before it was discovered, so its start time is unknown,
            // unless it was already removed from /proc when processing previous events
            if is_main_thread && !found {
                if self.finished_processes.remove(&process_id).is_none() {
                    self.registry.register_unverified_process(ProcessInfo {
                        pid: process_id,
                        kernel_thread: false,
                        command: UNVERIFIED_PROCESS_COMMAND.to_string(),
                        start_time: event.ts_boot_ticks,
                        exit_result: Some(event.exit_result),
                    });
                }
                return;
            }
        }

        if let Some(process) = self.processes.get_mut(&process_id)
            && event.ts_boot_ticks >= process.info.start_time
        {
            // main thread status is provisional, later thread exits override it
            if event.exit_result.is_err() || is_main_thread || process.info.exit_result.is_some() {
                process.info.exit_result = Some(event.exit_result);
            }
            if !is_main_thread {
                process.threads.remove(&ThreadId {
                    pid: event.process_tgid,
                    tid: event.process_pid,
                });
            }
            if tracked {
                self.update_process_by_pid(process_id).await;
            }
        }

        if is_main_thread
            && let Entry::Occupied(e) = self.unverified_processes.entry(process_id)
            && event.ts_boot_ticks >= e.get().info.start_time
        {
            let mut process = e.remove().info;
            process.exit_result = Some(event.exit_result);
            self.registry.register_unverified_process(process);
        }

        if is_main_thread {
            self.finished_processes.remove(&process_id);
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn handle_scan_finished(&mut self, join_result: Result<ScanResult, JoinError>) {
        let scan_touched = std::mem::take(&mut self.scan_touched);
        let task_result = match join_result {
            Ok(e) => e,
            Err(e) => return error!("process scan task failed: {e}"),
        };
        let scanned = match task_result {
            Ok(e) => e,
            Err(e) => return error!("process scan failed: {e}"),
        };

        let finished: Vec<_> = self
            .processes
            .extract_if(|e, _| !scanned.contains_key(e) && !scan_touched.contains(e))
            .map(|(_, e)| e.info)
            .collect();

        for process in finished {
            self.finished_processes.insert(process.pid, Instant::now());
            self.registry.unregister_process(&process);
        }

        for (pid, process) in scanned {
            if let Some(process) = process
                && !scan_touched.contains(&pid)
            {
                self.update_process_by_info(process);
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn handle_unverified_processes_timeout(&mut self) {
        self.finished_processes
            .retain(|_, e| e.elapsed() < UNVERIFIED_PROCESS_TIMEOUT);

        for (pid, e) in self
            .unverified_processes
            .extract_if(|_, e| e.created_at.elapsed() >= UNVERIFIED_PROCESS_TIMEOUT)
        {
            self.finished_processes.insert(pid, Instant::now());
            self.registry.register_unverified_process(e.info.clone());
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn update_process_by_pid(&mut self, pid: ProcessId) -> bool {
        // 1.
        // only confirmed disappearance is a proof of exit,
        // other read failures (permissions, transient errors) keep the tracked identity

        // 2.
        // we never remove processes with a missing exit code here. in worst case when we miss
        // main thread exit event and app still runs, it will be removed on the next full scan.
        // this is a compromise with missing error code (when /proc reaped before exit event).

        let task = tokio::task::spawn_blocking(move || {
            // `None` marks a process that disappeared or completed
            let (process, stat) = match Process::new(pid.0).and_then(|e| e.stat().map(|s| (e, s))) {
                Ok(e) => e,
                Err(ProcError::NotFound(_)) => return Ok(None),
                Err(e) => return Err(anyhow::Error::from(e)),
            };

            let info = parse_process_info_from_stat_if_alive(pid, stat);
            Ok(info.map(|e| VerifiedProcessInfo {
                info: e,
                threads: read_process_threads(&process),
            }))
        });

        let result = match task.await {
            Ok(e) => e,
            Err(e) => {
                warn!("task failed to read process {pid} info: {e}");
                return self.processes.contains_key(&pid);
            }
        };

        match result {
            Ok(Some(process)) => {
                self.update_process_by_info(process);
            }
            Ok(None) => {
                // entry without exit code is kept until exit event or full scan
                if let Entry::Occupied(e) = self.processes.entry(pid) {
                    if e.get().info.exit_result.is_none() {
                        return true;
                    }
                    let process = e.remove().info;
                    self.finished_processes.insert(pid, Instant::now());
                    self.registry.unregister_process(&process);
                }
                return false;
            }
            Err(e) => {
                warn!("failed to read process {pid} info: {e}");
                return self.processes.contains_key(&pid);
            }
        }
        true
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn update_process_by_info(&mut self, process: VerifiedProcessInfo) {
        // 1.
        // real start time never exceeds the unverified start time of the same process,
        // so a later start time always belongs to a newer pid owner

        // 2.
        // empty thread list means it could not be read, which is not a proof of thread exit

        if let Some(e) = self.unverified_processes.remove(&process.info.pid)
            && process.info.start_time > e.info.start_time
        {
            self.registry.register_unverified_process(e.info.clone());
        }

        match self.processes.entry(process.info.pid) {
            Entry::Occupied(mut e) => {
                if e.get().info.start_time == process.info.start_time {
                    let tracked = e.into_mut();
                    tracked.info.command = process.info.command;
                    self.registry.update_process(&tracked.info);

                    if !process.threads.is_empty() {
                        tracked.threads = process.threads;
                    }
                } else {
                    let old = e.insert(process.clone());
                    self.registry.unregister_process(&old.info);
                    self.registry.register_process(&process.info);
                }
            }
            Entry::Vacant(e) => {
                e.insert(process.clone());
                self.registry.register_process(&process.info);
            }
        }
    }
}

/// Reads info about every running process
/// `None` marks a process whose info could not be read, which is not a proof of exit
fn scan_processes() -> ScanResult {
    let mut scanned = HashMap::new();
    for process in procfs::process::all_processes()? {
        let process = match process {
            Ok(e) => e,
            Err(ProcError::NotFound(_)) => continue,
            Err(e) => return Err(e.into()),
        };

        let pid = ProcessId(process.pid);
        match process.stat() {
            Ok(stat) => {
                if let Some(info) = parse_process_info_from_stat_if_alive(pid, stat) {
                    let threads = read_process_threads(&process);
                    scanned.insert(pid, Some(VerifiedProcessInfo { info, threads }));
                }
            }
            Err(ProcError::NotFound(_)) => {}
            Err(_) => {
                scanned.insert(pid, None);
            }
        }
    }
    Ok(scanned)
}

/// Reads all process threads
/// empty set marks a thread list that could not be fully read, which is not a proof of thread exit
fn read_process_threads(process: &Process) -> HashSet<ThreadId> {
    let mut threads = HashSet::new();
    let Ok(tasks) = process.tasks() else {
        return threads;
    };
    for task in tasks {
        let task = match task {
            Ok(e) => e,
            Err(ProcError::NotFound(_)) => continue,
            Err(_) => return HashSet::new(),
        };
        threads.insert(ThreadId {
            pid: task.pid,
            tid: task.tid,
        });
    }
    threads
}

/// Builds process info from its thread group main thread stat
/// `None` marks a thread group where every thread has exited
fn parse_process_info_from_stat_if_alive(pid: ProcessId, stat: Stat) -> Option<ProcessInfo> {
    // 1.
    // exited but unreaped process keeps a readable stat with the same start time,
    // so presence in procfs alone is not a proof of being alive

    // 2.
    // main thread that exited before other threads stays a zombie until the whole group exits,
    // it is still counted in num_threads, so live threads keep the count above one

    let exited = matches!(stat.state, 'Z' | 'X' | 'x');
    if exited && stat.num_threads <= 1 {
        return None;
    }

    Some(ProcessInfo {
        pid,
        kernel_thread: StatFlags::from_bits_truncate(stat.flags).contains(StatFlags::PF_KTHREAD),
        command: stat.comm,
        start_time: stat.starttime,
        exit_result: None,
    })
}
