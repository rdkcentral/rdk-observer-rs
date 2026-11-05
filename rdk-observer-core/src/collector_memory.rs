//! System and per-process memory collection.
//!
//! This module will provide functions to collect memory readings for a selected
//! process and system totals. `collector_worker` will pass registered identities
//! for individual reads or call these functions for each target in a full
//! process collection pass, according to configuration.
//!
//! Following ADR-0001, system readings will include `MemTotal` and `MemAvailable`
//! from `/proc/meminfo`. Process readings will use per-PID procfs measurements;
//! the exact fields and accounting rules remain to be defined.
//! Readings will preserve timestamps, identities, values, units, and unavailable
//! measurements for preparation and encoding by `output_worker`. System and
//! process readings will be correlated by timestamp and need not sum exactly.

use crate::output_model::{MemoryRecord, SystemMemoryRecord};
use anyhow::Context;
use procfs::process::Process;
use procfs::{Current, Meminfo};

/// Reads total and available system memory from `/proc/meminfo`.
///
/// Values are returned in bytes.
pub fn read_system_memory() -> anyhow::Result<SystemMemoryRecord> {
    let meminfo = Meminfo::current().context("failed to read /proc/meminfo")?;

    Ok(SystemMemoryRecord {
        total_bytes: Some(meminfo.mem_total),
        available_bytes: meminfo.mem_available,
    })
}

/// Reads virtual, resident, and proportional memory for a process or thread.
///
/// Values come from `/proc/<pid>/status` and `/proc/<pid>/smaps_rollup` and are
/// returned in bytes. If `smaps_rollup` is unavailable, proportional memory is
/// not returned.
pub fn read_process_memory(pid: i32) -> anyhow::Result<MemoryRecord> {
    let process = Process::new(pid).with_context(|| format!("failed to open process {pid}"))?;

    let status = process
        .status()
        .with_context(|| format!("failed to read /proc/{pid}/status"))?;

    let smaps_rollup = process.smaps_rollup().ok().and_then(|e| {
        let mut iter = e.memory_map_rollup.0.into_iter();
        iter.next().map(|memory_map| memory_map.extension.map)
    });

    let smaps_value = |name: &str| {
        smaps_rollup
            .as_ref()
            .and_then(|values| values.get(name))
            .copied()
    };

    Ok(MemoryRecord {
        virtual_bytes: status.vmsize.map(kibibytes_to_bytes),
        resident_bytes: status.vmrss.map(kibibytes_to_bytes),
        proportional_bytes: smaps_value("Pss"),
    })
}

fn kibibytes_to_bytes(value: u64) -> u64 {
    value.saturating_mul(1024)
}
