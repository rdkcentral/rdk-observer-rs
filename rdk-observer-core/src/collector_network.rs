//! Network activity collection.
//!
//! This module will provide functions called by `collector_worker` according to
//! the configured scope. System readings may include traffic counters per
//! interface. Per-process readings will require a source that can attribute
//! traffic to a registered target; interface totals will not be presented as
//! an individual process's traffic.
//!
//! Interfaces are ordered deterministically by name and mapped to
//! `SystemNetworkRecord` entries containing raw counters and optional rates.
//! Process-level network accounting remains deferred. Readings preserve
//! timestamps, counters, and units for preparation and encoding by `output_worker`.

use crate::output_model::{
    SystemNetworkCounters, SystemNetworkInterfaceIdentity, SystemNetworkRecord,
};
use anyhow::Context;
use procfs::net::dev_status;

/// Reads network traffic counters per interface from `/proc/net/dev`.
///
/// Loopback interface (`lo`) is excluded.
/// Counters are cumulative raw values; rates are calculated over consecutive samples.
pub fn read_system_network() -> anyhow::Result<Vec<SystemNetworkRecord>> {
    let statuses = dev_status().context("Failed to read network stats from procfs")?;
    let mut records = Vec::new();

    for (interface_name, status) in statuses {
        if interface_name == "lo" {
            continue;
        }

        // todo: gather correct if_index and ns_inode
        let interface_identity = SystemNetworkInterfaceIdentity {
            network_namespace_inode: 0,
            interface_index: 0,
            name: interface_name,
        };

        let counters = SystemNetworkCounters {
            rx_bytes: Some(status.recv_bytes),
            tx_bytes: Some(status.sent_bytes),
            rx_packets: Some(status.recv_packets),
            tx_packets: Some(status.sent_packets),
            rx_errors: Some(status.recv_errs),
            tx_errors: Some(status.sent_errs),
            rx_dropped: Some(status.recv_drop),
            tx_dropped: Some(status.sent_drop),
        };

        records.push(SystemNetworkRecord {
            interface_identity,
            counters,
            rates: None,
        });
    }

    records.sort_by(|a, b| a.interface_identity.cmp(&b.interface_identity));
    Ok(records)
}
