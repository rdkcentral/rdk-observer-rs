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

use crate::collector_cpu::{read_process_cpu, read_system_cpu};
use crate::collector_io::{read_process_io, read_system_io};
use crate::collector_memory::{read_process_memory, read_system_memory};
use crate::output_model::{
    BlockDeviceIdentity, CpuCounters, CpuUtilization, IoRates, ObservationRecord, ProcessIdentity,
    ResourceRecord, StorageCounters, SystemCpuCounters, SystemRecord, SystemResourceRecord,
    SystemStorageCounters,
};
use crate::process_worker::{ProcessEvent, ProcessId, ProcessInfo, ProcessWorker};
use rustix::time::{ClockId, clock_gettime};
use std::collections::HashMap;
use std::time::Duration;
use tokio::select;
use tokio::sync::Semaphore;
use tokio::task::{AbortHandle, JoinSet, spawn_blocking};
use tokio::time::{Instant, MissedTickBehavior};
use tracing::{error, info, instrument, trace, warn};

/// Interval at which to collect system stats
const SYSTEM_COLLECTION_INTERVAL: Duration = Duration::from_secs(10);

/// Interval at which to collect each process stats for first few samples
const PROCESS_COLLECTION_INTERVAL_INITIAL: Duration = Duration::from_secs(1);

/// Interval at which to collect each process stats for longer running processes
const PROCESS_COLLECTION_INTERVAL_ONGOING: Duration = Duration::from_secs(60);

/// Safeguard to limit number of processes that can be collected at the same time
static PROCESS_COLLECTION_CONCURRENCY: Semaphore = Semaphore::const_new(4);

#[derive(Debug)]
pub enum CollectorEvent {
    System(SystemRecord),
    Process((ProcessId, ObservationRecord)),
}

/// Worker that collect system/process stats with some interval
pub struct CollectorWorker {
    /// events produced to outside consumer
    output_events_tx: tokio::sync::mpsc::Sender<CollectorEvent>,
    /// events produced from process worker to collector coordinator
    process_events_rx: tokio::sync::mpsc::Receiver<ProcessEvent>,
    /// per_pid workers
    per_pid_workers: HashMap<ProcessId, AbortHandle>,
    /// collector tasks join set
    join_set: JoinSet<()>,
}

impl CollectorWorker {
    ////////////////////////////////////////////////////////////////////////////////
    #[instrument(skip_all, "collector")]
    pub async fn run(events_tx: tokio::sync::mpsc::Sender<CollectorEvent>) {
        let (process_events_tx, process_events_rx) = tokio::sync::mpsc::channel(16);

        let mut this = Self {
            output_events_tx: events_tx.clone(),
            process_events_rx,
            per_pid_workers: Default::default(),
            join_set: Default::default(),
        };

        this.join_set.spawn(ProcessWorker::run(process_events_tx));
        this.join_set.spawn(SystemWorker::run(events_tx.clone()));

        loop {
            select! {
                event = this.process_events_rx.recv() => {
                    this.handle_process_event(event).await;
                }
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
    async fn handle_process_event(&mut self, event: Option<ProcessEvent>) {
        let Some(event) = event else {
            for (_, task) in self.per_pid_workers.drain() {
                task.abort();
            }
            let (tx, rx) = tokio::sync::mpsc::channel(self.process_events_rx.max_capacity());
            self.process_events_rx = rx;
            self.join_set.spawn(ProcessWorker::run(tx));
            return warn!("process worker restarted");
        };

        match event {
            ProcessEvent::ProcessStarted(e) => self.handle_process_event_started(e).await,
            ProcessEvent::ProcessFinished(e) => self.handle_process_event_finished(e).await,
            ProcessEvent::ProcessUnverified(_) => {
                // todo implement when unverified events will be available
            }
            ProcessEvent::ThreadStarted(_) => {
                // todo implement when thread collector will be ready
            }
            ProcessEvent::ThreadFinished(_) => {
                // todo implement when thread collector will be ready
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_process_event_started(&mut self, process: ProcessInfo) {
        trace!("started: {process:?}");

        let process_pid = process.pid;
        let future = PerPidWorker::run(process, self.output_events_tx.clone());
        let handle = self.join_set.spawn(future);

        if let Some(task) = self.per_pid_workers.insert(process_pid, handle) {
            task.abort();
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_process_event_finished(&mut self, process: ProcessInfo) {
        trace!("finished: {process:?}");

        if let Some(task) = self.per_pid_workers.remove(&process.pid) {
            task.abort();
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
struct SystemWorker {
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
    async fn run(events_tx: tokio::sync::mpsc::Sender<CollectorEvent>) {
        trace!("started");

        let mut interval = tokio::time::interval(SYSTEM_COLLECTION_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut this = Self {
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

        let mut records = spawn_blocking(read_system_io).await??;
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

        let mut record = spawn_blocking(read_system_cpu).await??;
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

        let record = spawn_blocking(read_system_memory).await??;
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
    pending_events: Vec<CollectorEvent>,
    process: ProcessInfo,
    last_cpu_record: Option<(Duration, CpuCounters)>,
    last_storage_record: Option<(Duration, StorageCounters)>,
}

impl PerPidWorker {
    ////////////////////////////////////////////////////////////////////////////////
    #[instrument(skip_all, "collector/process", fields(pid = %process.pid))]
    async fn run(process: ProcessInfo, events_tx: tokio::sync::mpsc::Sender<CollectorEvent>) {
        trace!("started");

        let mut frame_index = 0u32;
        let mut interval = tokio::time::interval(PROCESS_COLLECTION_INTERVAL_INITIAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut this = Self {
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

            for event in this.pending_events.drain(..) {
                if events_tx.send(event).await.is_err() {
                    return warn!("events_tx is closed, stopping");
                }
            }

            // first few frames are collected with fast interval
            // all consecutive frames are collected with slower interval
            frame_index = frame_index.saturating_add(1);
            if frame_index == 2 {
                let jitter_range = PROCESS_COLLECTION_INTERVAL_ONGOING;
                let jitter_in = rand::random::<f32>() * jitter_range.as_secs_f32();
                let jitter_at = Instant::now() + Duration::from_secs_f32(jitter_in);

                interval = tokio::time::interval_at(jitter_at, jitter_range);
                interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn collect_io(&mut self) -> anyhow::Result<()> {
        trace!("collecting io");

        let pid = self.process.pid.0;
        let mut record = spawn_blocking(move || read_process_io(pid)).await??;
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

        let pid = self.process.pid.0;
        let start_time = self.process.start_time;
        let mut record = spawn_blocking(move || read_process_cpu(pid, start_time)).await??;
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

        let pid = self.process.pid.0;
        let record = spawn_blocking(move || read_process_memory(pid)).await??;
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
