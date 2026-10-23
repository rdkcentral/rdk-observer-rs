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

use crate::process_worker::ProcessWorker;
use tokio::task::JoinSet;
use tracing::info;

/// Run the observer until shutdown.
///
/// This is the library entry point called by the executable. Worker startup,
/// channels, observer mode transitions, and shutdown will be implemented here.
pub async fn run() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);

    let mut join_set = JoinSet::new();
    join_set.spawn(ProcessWorker::run(tx));

    while let Some(event) = rx.recv().await {
        info!("{:?}", event);
    }
}
