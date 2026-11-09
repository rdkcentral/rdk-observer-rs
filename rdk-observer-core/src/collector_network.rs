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

use crate::output_model::{InterfaceCounters, SystemNetworkRecord};
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

        let counters = InterfaceCounters {
            rx_bytes: status.recv_bytes,
            tx_bytes: status.sent_bytes,
            rx_packets: status.recv_packets,
            tx_packets: status.sent_packets,
            rx_errors: status.recv_errs,
            tx_errors: status.sent_errs,
            rx_dropped: status.recv_drop,
            tx_dropped: status.sent_drop,
        };

        records.push(SystemNetworkRecord {
            name: interface_name,
            counters,
            rates: None,
        });
    }

    records.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(records)
}
