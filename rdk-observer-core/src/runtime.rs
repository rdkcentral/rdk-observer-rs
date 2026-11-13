//! Task orchestration, scheduling, and shutdown.
//!
//! This module will receive configuration built from input parameters and start
//! the required Tokio workers. Process monitoring will use `process_discovery`
//! and `process_worker` to establish and maintain targets in `process_registry`.
//! Startup will use `normal-observer-mode`: subscribe to available netlink
//! process connector events before the initial procfs inventory scan and keep
//! reconciliation scans running every 30 seconds. No eBPF programs will be
//! loaded or attached in normal mode.
//!
//! Only anomaly triggers will activate `forensic-observer-mode`. The runtime
//! will validate process or thread identities and coordinate targeted Aya/eBPF
//! sessions under configured capture duration and resource limits. Netlink
//! discovery, reconciliation, and routine collectors will continue throughout.
//! Coalesce repeated triggers and release session resources on completion,
//! target exit, failed initialization, or shutdown, retaining shared resources
//! while other sessions need them. Return to normal mode after all sessions end.
//! If forensic capture is unavailable, report it and continue normal observation.
//! Anomaly trigger sources, probes, and detailed session policies remain to be
//! defined, following ADR-0003.
//!
//! Enabled workers will start in this order:
//!
//! 1. `process_*`: start discovery and coordination to establish registered targets.
//! 2. `collector_*`: start process collection tasks and requested system collection.
//! 3. `output_*`: initialize storage and start observation preparation and encoding.
//!
//! Collection tasks will wait for output readiness before producing observations.
//! Discovery and collection coordination will continue tracking targets while
//! output initializes.
//!
//! `process_worker` will coordinate collection for registered targets, using
//! `collector_worker` tasks for individual processes or full-list collection
//! requests. The runtime will start independent system collection when totals
//! are requested. Collection will use only the enabled resource types.
//!
//! Collection tasks will exchange collection instructions and status with
//! `process_worker` and send observations to `output_worker` through channels.
//! The output worker will prepare Protobuf records and store them through
//! `output_storage`. Consumption or delivery of buffered records will be
//! defined separately.
//!
//! The runtime will connect workers, provide shared concurrency limits, monitor
//! failures, and manage startup and shutdown. Pending record handling and
//! failure policies remain to be defined.

use crate::collector_worker::{CollectorEvent, CollectorWorker};
use crate::process_registry::{ProcessRegistry, ProcessRegistryWriter};
use crate::process_worker::ProcessWorker;
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;
use tokio::select;
use tokio::task::JoinSet;
use tracing::{debug, error, info, instrument};

////////////////////////////////////////////////////////////////////////////////
const WORKER_RESTART_DELAY: Duration = Duration::from_secs(1);

////////////////////////////////////////////////////////////////////////////////
/// Configuration of the [Runtime]
///
/// Process filtering is done in exclude->include order (include has higher priority)
#[derive(Debug)]
pub struct RuntimeConfig {
    /// Interval at which to collect system stats
    pub system_collection_interval: Duration,
    /// Interval at which to collect each process stats for first few samples
    pub process_collection_interval_initial: Duration,
    /// Interval at which to collect each process stats for longer running processes
    pub process_collection_interval_ongoing: Duration,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            system_collection_interval: Duration::from_secs(10),
            process_collection_interval_initial: Duration::from_secs(1),
            process_collection_interval_ongoing: Duration::from_secs(60),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Runtime context that can be shared between workers
///
/// It allows any component to access shared global state, like config
#[derive(Clone)]
#[repr(transparent)]
pub struct RuntimeContext(Arc<RuntimeContextShared>);

impl Deref for RuntimeContext {
    type Target = RuntimeContextShared;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct RuntimeContextShared {
    pub config: RuntimeConfig,
    pub registry: ProcessRegistry,
}

////////////////////////////////////////////////////////////////////////////////
/// Observer runtime
///
/// Responsible for orchestrating process workers, collectors and storage
pub struct Runtime {
    context: RuntimeContext,
    registry_writer: ProcessRegistryWriter,
}

impl Runtime {
    ////////////////////////////////////////////////////////////////////////////////
    /// Create new runtime instance
    pub fn new(config: RuntimeConfig) -> Self {
        let (registry, registry_writer) = ProcessRegistry::new();
        let context = RuntimeContext(Arc::new(RuntimeContextShared { config, registry }));

        Self {
            context,
            registry_writer,
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Return context for this runtime
    pub fn context(&self) -> RuntimeContext {
        self.context.clone()
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Return process registry used by this runtime
    pub fn registry(&self) -> ProcessRegistry {
        self.context.registry.clone()
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Runs the runtime to completion
    ///
    /// This is the library entry point called by the executable. Worker startup,
    /// channels, observer mode transitions, and shutdown will be implemented here.
    #[instrument(skip_all, "runtime")]
    pub async fn run(self) -> anyhow::Result<()> {
        debug!("runtime started");

        let (tx, mut rx) = tokio::sync::mpsc::channel(64);

        let mut process_workers = JoinSet::new();
        process_workers.spawn(self.build_process_worker(None));

        let mut collector_workers = JoinSet::new();
        collector_workers.spawn(self.build_collector_worker(tx.clone(), None));

        loop {
            select! {
                Some(event) = rx.recv() => {
                    // todo: link to output collector when implemented
                    info!("{event:?}");
                }
                result = process_workers.join_next() => {
                    if let Some(Err(e)) = result {
                        error!("process worker failed: {e}");
                    }

                    // previous worker state is not valid anymore, subscribers observe removal
                    self.registry_writer.clear_all_processes();

                    info!("process worker restarting in {WORKER_RESTART_DELAY:?}");
                    process_workers.spawn(self.build_process_worker(Some(WORKER_RESTART_DELAY)));
                }
                result = collector_workers.join_next() => {
                    if let Some(Err(e)) = result {
                        error!("collector worker failed: {e}");
                    }
                    info!("collector worker restarting in {WORKER_RESTART_DELAY:?}");
                    collector_workers.spawn(self.build_collector_worker(tx.clone(), Some(WORKER_RESTART_DELAY)));
                }
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn build_process_worker(
        &self,
        start_delay: Option<Duration>,
    ) -> impl Future<Output = ()> + 'static {
        let registry = self.registry_writer.clone();

        async move {
            if let Some(start_delay) = start_delay {
                tokio::time::sleep(start_delay).await;
            }
            ProcessWorker::run(registry).await
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn build_collector_worker(
        &self,
        tx: tokio::sync::mpsc::Sender<CollectorEvent>,
        start_delay: Option<Duration>,
    ) -> impl Future<Output = ()> + 'static {
        let context = self.context.clone();

        async move {
            if let Some(start_delay) = start_delay {
                tokio::time::sleep(start_delay).await;
            }
            CollectorWorker::run(context, tx).await
        }
    }
}
