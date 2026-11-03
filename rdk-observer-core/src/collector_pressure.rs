//! Resource pressure collection.
//!
//! This module will provide functions for CPU, memory, and I/O pressure readings,
//! called by `collector_worker` according to configuration. Process targets will
//! come from `process_registry` where the source supports process attribution.
//! System pressure will remain distinguishable from process measurements.
//!
//! Sources, fields, and supported scopes remain to be defined by an ADR.
//! Unsupported process measurements will be represented as unavailable.
//! Readings will retain timestamps, identities, values, and units for processing
//! and encoding by `output_worker`.

use procfs::{CpuPressure, Current, IoPressure, MemoryPressure};

use crate::output_model::{PressureResourceRecord, PressureStallRecord, SystemPressureRecord};

/// Reads system CPU, memory, and I/O pressure from `/proc/pressure`.
///
/// A resource is unavailable when its PSI file cannot be read.
pub fn read_system_pressure() -> anyhow::Result<SystemPressureRecord> {
    let cpu = match CpuPressure::current() {
        Ok(pressure) => Some(pressure_resource_record(pressure.some, pressure.full)),
        Err(error) => {
            tracing::warn!(error = ?error, "failed to read /proc/pressure/cpu");
            None
        }
    };

    let memory = match MemoryPressure::current() {
        Ok(pressure) => Some(pressure_resource_record(pressure.some, pressure.full)),
        Err(error) => {
            tracing::warn!(error = ?error, "failed to read /proc/pressure/memory");
            None
        }
    };

    let io = match IoPressure::current() {
        Ok(pressure) => Some(pressure_resource_record(pressure.some, pressure.full)),
        Err(error) => {
            tracing::warn!(error = ?error, "failed to read /proc/pressure/io");
            None
        }
    };

    Ok(SystemPressureRecord { cpu, memory, io })
}

fn pressure_resource_record(
    some: procfs::PressureRecord,
    full: procfs::PressureRecord,
) -> PressureResourceRecord {
    PressureResourceRecord {
        some: PressureStallRecord {
            avg10_percent: some.avg10,
            avg60_percent: some.avg60,
            avg300_percent: some.avg300,
            total_microseconds: some.total,
        },
        full: PressureStallRecord {
            avg10_percent: full.avg10,
            avg60_percent: full.avg60,
            avg300_percent: full.avg300,
            total_microseconds: full.total,
        },
    }
}
