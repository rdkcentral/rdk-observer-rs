//! System and per-process I/O counter collection.
//!
//! This module will provide functions to read I/O counters for a selected
//! process and system devices. `collector_worker` will pass registered identities
//! for individual reads or call these functions across the selected list.
//! The exact per-PID fields and accounting rules remain to be defined.
//!
//! Following ADR-0001, system readings will retain raw per-device counters from
//! `/proc/diskstats` and identify devices by major/minor number and name. Device
//! selection must avoid double-counting disks and their partitions. System and
//! process I/O accounting will remain distinguishable in the observations.
//!
//! Readings will retain timestamps, identities, counters, units, and unavailable
//! values. `collector_worker` will derive rates from consecutive valid samples
//! when configured; counter resets, missing targets, or identity changes will
//! invalidate affected deltas. Filesystem capacity and free space are outside
//! ADR-0001's scope.

use crate::output_model::{
    StorageCounters, StorageRecord, SystemBlockDeviceIdentity, SystemStorageCounters,
    SystemStorageRecord,
};
use anyhow::Context;
use procfs::process::Process;
use procfs::{DiskStat, diskstats};
use tracing::warn;

/// Reads storage I/O counters for block devices from `/proc/diskstats`.
///
/// Partition devices are skipped using sysfs. Rates are not calculated here.
pub fn read_system_io() -> anyhow::Result<Vec<SystemStorageRecord>> {
    let disk_stats = diskstats().context("failed to read /proc/diskstats")?;
    let mut records = Vec::new();

    for disk_stat in disk_stats {
        if !is_disk_valid_non_partition(&disk_stat) {
            continue;
        }

        records.push(SystemStorageRecord {
            device_identity: SystemBlockDeviceIdentity {
                major: disk_stat.major.cast_unsigned(),
                minor: disk_stat.minor.cast_unsigned(),
                name: disk_stat.name,
            },
            counters: SystemStorageCounters {
                read_operations: Some(disk_stat.reads),
                write_operations: Some(disk_stat.writes),
                read_sectors: Some(disk_stat.sectors_read),
                write_sectors: Some(disk_stat.sectors_written),
            },
            rates: None,
        });
    }
    Ok(records)
}

/// Reads process-wide storage I/O counters from `/proc/<pid>/io`.
///
/// Rates are not calculated here.
pub fn read_process_io(pid: i32) -> anyhow::Result<StorageRecord> {
    let process = Process::new(pid).with_context(|| format!("failed to open process {pid}"))?;

    let io = process
        .io()
        .with_context(|| format!("failed to read /proc/{pid}/io"))?;

    Ok(StorageRecord {
        counters: StorageCounters {
            read_bytes: Some(io.read_bytes),
            write_bytes: Some(io.write_bytes),
            read_syscalls: Some(io.syscr),
            write_syscalls: Some(io.syscw),
        },
        rates: None,
    })
}

/// Checks if disk represented by [DiskStat] is valid and not a partition
fn is_disk_valid_non_partition(disk_stat: &DiskStat) -> bool {
    let block_path = format!("/sys/dev/block/{}:{}", disk_stat.major, disk_stat.minor);
    match std::fs::exists(&block_path) {
        Ok(block_exists) => {
            if !block_exists {
                return false;
            }
        }
        Err(e) => {
            warn!("failed to check block {block_path}: {e}");
            return false;
        }
    }

    let partition_path = format!("{block_path}/partition");
    match std::fs::exists(&partition_path) {
        Ok(partition_exists) => !partition_exists,
        Err(e) => {
            warn!("failed to check partition {partition_path}: {e}");
            false
        }
    }
}
