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

use crate::process_discovery::NetlinkProcessDiscovery;
use crate::process_registry::ProcessRegistry;
use crate::runtime_components::{DefaultRuntimeComponents, RuntimeComponents};
use anyhow::Context;
use std::collections::HashSet;
use std::fmt::Debug;
use std::sync::Arc;
use tokio::select;
use tokio::task::JoinSet;
use tracing::{debug, info, instrument};

/// Configuration of the [Runtime]
///
/// Process filtering is done in exclude->include order (include has higher priority)
#[derive(Debug, Default)]
pub struct RuntimeConfig {
    /// Include the processes with this PID
    pub include_pids: HashSet<u32>,
    /// Include processes with this command name; repeat to add more
    pub include_commands: HashSet<String>,
    /// Include RDK processes from the configured selection list
    pub include_rdk_processes: bool,
    /// Exclude kernel threads
    pub exclude_kernel_threads: bool,
    /// Exclude userspace processes
    pub exclude_userspace_processes: bool,
    /// Components provider, see [RuntimeComponents]
    pub components: Option<Box<dyn RuntimeComponents>>,
}

/// Runtime context that can be shared between workers
///
/// It allows any component to access shared global state, like config
pub(super) struct RuntimeContext {
    pub config: RuntimeConfig,
    pub components: Box<dyn RuntimeComponents>,
    pub process_registry: ProcessRegistry,
}

/// Run the observer until shutdown.
///
/// This is the library entry point called by the executable. Worker startup,
/// channels, observer mode transitions, and shutdown will be implemented here.
#[instrument(skip_all, "runtime")]
pub async fn run(mut config: RuntimeConfig) -> anyhow::Result<()> {
    debug!(?config, "initializing runtime");
    let components = config.components.take();
    let context = Arc::new(RuntimeContext {
        config,
        components: components.unwrap_or_else(|| Box::new(DefaultRuntimeComponents)),
        process_registry: Default::default(),
    });

    debug!("staring workers");
    let mut join_set = JoinSet::new();
    join_set.spawn(stub_worker(context));

    info!("runtime started");
    join_set
        .join_next()
        .await
        .context("runtime has no workers")?
        .context("runtime worker has failed")?
}

/// todo: replace with actual workers when process/collector workers are ready
async fn stub_worker(_context: Arc<RuntimeContext>) -> anyhow::Result<()> {
    let mut discovery = NetlinkProcessDiscovery::default();

    loop {
        select! {
            _ = discovery.connected.changed() => {
                info!("discovery connected: {}", *discovery.connected.borrow_and_update());
            }
            _ = discovery.refresh.notified() => {
                info!("discovery requested refresh");
            }
            event = discovery.events.recv() => {
                info!("discovery event: {event:?}");
            }
        }
    }
}
