//! System and per-process CPU counter collection.
//!
//! This module will provide functions to read aggregate CPU counters from
//! `/proc/stat` for system totals, following ADR-0001, and user and kernel CPU
//! time counters from `/proc/<pid>/stat` for a selected process.
//!
//! `collector_worker` will call these functions for individual registered
//! targets, a full pass over the selected list, or system totals, according to
//! configuration. Process reads will receive the PID and expected start time.
//!
//! Readings will retain timestamps, identities, raw counters, and units. Exited
//! or replaced targets will not produce a valid sample for the original process.
//! `collector_worker` will derive CPU utilization from consecutive valid samples
//! when configured. A first sample will establish a baseline; counter resets or
//! identity changes will invalidate the delta. System counters will provide
//! context when system and process observations are both enabled.

use anyhow::{Context, anyhow};
use procfs::process::Process;
use procfs::{CurrentSI, KernelStats, ticks_per_second};

use crate::output_model::{CpuCounters, CpuRecord, SystemCpuCounters, SystemCpuRecord};

/// Reads aggregate CPU counters for the system from `/proc/stat`.
///
/// Counters are returned in clock ticks.
/// Utilization is not calculated here.
pub fn read_system_cpu() -> anyhow::Result<SystemCpuRecord> {
    let kernel_stats = KernelStats::current().context("failed to read /proc/stat")?;

    Ok(SystemCpuRecord {
        clock_ticks_per_second: ticks_per_second(),
        counters: SystemCpuCounters {
            user_ticks: Some(kernel_stats.total.user),
            nice_ticks: Some(kernel_stats.total.nice),
            system_ticks: Some(kernel_stats.total.system),
            idle_ticks: Some(kernel_stats.total.idle),
            iowait_ticks: kernel_stats.total.iowait,
            irq_ticks: kernel_stats.total.irq,
            softirq_ticks: kernel_stats.total.softirq,
            steal_ticks: kernel_stats.total.steal,
            guest_ticks: kernel_stats.total.guest,
            guest_nice_ticks: kernel_stats.total.guest_nice,
        },
        utilization: None,
    })
}

/// Reads CPU counters for a process or thread from `/proc/<pid>/stat`.
///
/// Checks `expected_starttime` to reject a reused PID or TID. Counters are
/// returned in clock ticks.
/// Utilization is not calculated here.
pub fn read_process_cpu(pid: i32, expected_starttime: u64) -> anyhow::Result<CpuRecord> {
    read_process_cpu_with_state(pid, expected_starttime).map(|(record, _)| record)
}

/// Reads CPU counters and state from the same identity-validated stat sample.
pub(crate) fn read_process_cpu_with_state(
    pid: i32,
    expected_starttime: u64,
) -> anyhow::Result<(CpuRecord, char)> {
    let process = Process::new(pid).with_context(|| format!("failed to open process {pid}"))?;

    let stat = process
        .stat()
        .with_context(|| format!("failed to read /proc/{pid}/stat"))?;

    if stat.starttime != expected_starttime {
        return Err(anyhow!(
            "starttime mismatch for PID {pid}: expected {expected_starttime}, found {}",
            stat.starttime
        ));
    }

    Ok((
        CpuRecord {
            clock_ticks_per_second: ticks_per_second(),
            counters: Some(CpuCounters {
                user_ticks: Some(stat.utime),
                system_ticks: Some(stat.stime),
            }),
            utilization: None,
            forensic: None,
        },
        stat.state,
    ))
}
