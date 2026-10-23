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

use crate::process_worker::{ProcessEvent, ProcessId, ProcessInfo, ProcessWorker};
use std::collections::HashMap;
use std::time::Duration;
use tokio::select;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::MissedTickBehavior;
use tokio_util::task::AbortOnDropHandle;
use tracing::{instrument, trace, warn};

/// Interval at which to collect system stats
const SYSTEM_COLLECTION_INTERVAL: Duration = Duration::from_secs(60);

/// Interval at which to collect each process stats
const PROCESS_COLLECTION_INTERVAL: Duration = Duration::from_secs(60);

/// Safeguard to limit number of processes that can be collected at the same time
const PROCESS_COLLECTION_CONCURRENCY: Semaphore = Semaphore::const_new(8);

#[derive(Debug)]
enum CollectorWorkerEvent {
    System(CollectorSystemEvent),
    Process(CollectorProcessEvent),
}

#[derive(Debug)]
struct CollectorSystemEvent;

#[derive(Debug)]
struct CollectorProcessEvent {
    process_info: ProcessInfo,
}

pub struct CollectorWorker {
    /// events produced to outside consumer
    output_events_tx: tokio::sync::mpsc::Sender<String>,
    /// events produced from system/per-pid workers to collector coordinator
    collector_events_tx: tokio::sync::mpsc::Sender<CollectorWorkerEvent>,
    /// events produced from process worker to collector coordinator
    process_events_tx: tokio::sync::mpsc::Sender<ProcessEvent>,
    /// per_pid workers
    per_pid_workers: HashMap<ProcessId, (ProcessInfo, AbortOnDropHandle<()>)>,
    /// collector tasks join set
    join_set: JoinSet<()>,
}

impl CollectorWorker {
    ////////////////////////////////////////////////////////////////////////////////
    #[instrument(skip_all, "collector")]
    pub async fn run(output_events_tx: tokio::sync::mpsc::Sender<String>) {
        let (process_events_tx, mut process_events_rx) = tokio::sync::mpsc::channel(16);
        let (collector_events_tx, mut collector_events_rx) = tokio::sync::mpsc::channel(16);

        let mut this = Self {
            output_events_tx,
            collector_events_tx: collector_events_tx.clone(),
            process_events_tx: process_events_tx.clone(),
            per_pid_workers: Default::default(),
            join_set: Default::default(),
        };

        this.join_set.spawn(ProcessWorker::run(process_events_tx));
        this.join_set.spawn(run_system_worker(collector_events_tx));

        loop {
            select! {
                event = process_events_rx.recv() => {
                    this.handle_process_event(event).await;
                }
                event = collector_events_rx.recv() => {
                    this.handle_collector_event(event).await;
                }
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_process_event(&mut self, event: Option<ProcessEvent>) {
        let Some(event) = event else {
            return warn!("process worker channel closed");
        };

        match event {
            ProcessEvent::Started(e) => self.handle_process_event_started(e).await,
            ProcessEvent::Finished(e) => self.handle_process_event_finished(e).await,
            ProcessEvent::Unverified(e) => self.handle_process_event_unverified(e).await,
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_process_event_started(&mut self, process: ProcessInfo) {
        trace!("started: {process:?}");

        let task = AbortOnDropHandle::new(tokio::spawn(run_process_worker(
            process.clone(),
            self.collector_events_tx.clone(),
        )));

        self.per_pid_workers.insert(process.pid, (process, task));
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_process_event_finished(&mut self, process: ProcessInfo) {
        trace!("finished: {process:?}");

        self.per_pid_workers.remove(&process.pid);
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_process_event_unverified(&mut self, process: ProcessInfo) {
        trace!("unverified: {process:?}");
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_collector_event(&mut self, event: Option<CollectorWorkerEvent>) {
        let Some(event) = event else {
            return warn!("per-pid collector channel closed");
        };

        trace!("event: {event:?}");

        // TODO replace with real output worker event
        let _ = self.output_events_tx.send(format!("{event:?}")).await;
    }
}

////////////////////////////////////////////////////////////////////////////////
#[instrument(skip_all, "collector/system")]
async fn run_system_worker(events_tx: tokio::sync::mpsc::Sender<CollectorWorkerEvent>) {
    let mut interval = tokio::time::interval(SYSTEM_COLLECTION_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        trace!("started");
        // TODO run system collectors here

        interval.tick().await;
    }
}

////////////////////////////////////////////////////////////////////////////////
#[instrument(skip_all, "collector/process", fields(pid = %process.pid))]
async fn run_process_worker(
    process: ProcessInfo,
    events_tx: tokio::sync::mpsc::Sender<CollectorWorkerEvent>,
) {
    let mut interval = tokio::time::interval(PROCESS_COLLECTION_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        if let Ok(_permit) = PROCESS_COLLECTION_CONCURRENCY.acquire().await {
            trace!("started for {}", process.command);
            // TODO run per-pid collectors here
        }
        interval.tick().await;
    }
}
