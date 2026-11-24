//! Registry of the latest system collection state.
//!
//! `collector_worker` updates one resource after each successful system collection.
//! System resources are collected independently and do not form one atomic measurement.

use crate::output_model::{
    SystemCpuRecord, SystemMemoryRecord, SystemNetworkRecord, SystemPressureRecord, SystemRecord,
    SystemResourceRecord, SystemStorageRecord,
};
use parking_lot::RwLock;
use std::{sync::Arc, time::Duration};

////////////////////////////////////////////////////////////////////////////////
/// Latest system metrics.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemRegistrySnapshot {
    pub last_io_records: Option<(Duration, Vec<SystemStorageRecord>)>,
    pub last_cpu_record: Option<(Duration, SystemCpuRecord)>,
    pub last_memory_record: Option<(Duration, SystemMemoryRecord)>,
    pub last_network_records: Option<(Duration, Vec<SystemNetworkRecord>)>,
    pub last_pressure_record: Option<(Duration, SystemPressureRecord)>,
}

////////////////////////////////////////////////////////////////////////////////
/// Shared storage for the latest system metrics.
#[derive(Debug, Clone, Default)]
pub struct SystemRegistry {
    state: Arc<RwLock<SystemRegistrySnapshot>>,
}

impl SystemRegistry {
    ////////////////////////////////////////////////////////////////////////////////
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Returns a copy of the latest system metrics.
    pub fn get_snapshot(&self) -> SystemRegistrySnapshot {
        self.state.read().clone()
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Stores the latest metric from `record`.
    pub(crate) fn update(&self, record: &SystemRecord) {
        let timestamp = Duration::from_nanos(record.timestamp_unix_ns);
        let resource = record.resource.clone();
        let mut state = self.state.write();

        match resource {
            SystemResourceRecord::Cpu(value) => {
                state.last_cpu_record = Some((timestamp, value));
            }
            SystemResourceRecord::Memory(value) => {
                state.last_memory_record = Some((timestamp, value));
            }
            SystemResourceRecord::Storage(value) => {
                state.last_io_records = Some((timestamp, value));
            }
            SystemResourceRecord::Network(value) => {
                state.last_network_records = Some((timestamp, value));
            }
            SystemResourceRecord::Pressure(value) => {
                state.last_pressure_record = Some((timestamp, value));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output_model::{CpuUtilization, SystemCpuCounters};

    fn record(timestamp_unix_ns: u64, resource: SystemResourceRecord) -> SystemRecord {
        SystemRecord {
            timestamp_unix_ns,
            timestamp_monotonic_ns: 0,
            resource,
        }
    }

    #[test]
    fn updates_resources_independently() {
        let registry = SystemRegistry::new();
        let cpu = record(
            10,
            SystemResourceRecord::Cpu(SystemCpuRecord {
                clock_ticks_per_second: 100,
                counters: SystemCpuCounters::default(),
                utilization: Some(CpuUtilization {
                    interval_ns: 1_000_000_000,
                    percent: 12.5,
                }),
            }),
        );
        let memory = record(
            20,
            SystemResourceRecord::Memory(SystemMemoryRecord {
                total_bytes: Some(1024),
                available_bytes: Some(512),
            }),
        );

        registry.update(&cpu);
        registry.update(&memory);

        let snapshot = registry.get_snapshot();
        let (cpu_timestamp, cpu) = snapshot.last_cpu_record.unwrap();
        let (memory_timestamp, memory) = snapshot.last_memory_record.unwrap();
        assert_eq!(cpu_timestamp, Duration::from_nanos(10));
        assert_eq!(memory_timestamp, Duration::from_nanos(20));
        assert_eq!(cpu.utilization.unwrap().percent, 12.5);
        assert_eq!(memory.available_bytes, Some(512));
        assert!(snapshot.last_io_records.is_none());
        assert!(snapshot.last_network_records.is_none());
        assert!(snapshot.last_pressure_record.is_none());
    }

    #[test]
    fn newer_record_replaces_only_the_same_resource() {
        let registry = SystemRegistry::new();
        registry.update(&record(
            10,
            SystemResourceRecord::Memory(SystemMemoryRecord {
                total_bytes: Some(1024),
                available_bytes: Some(512),
            }),
        ));
        registry.update(&record(
            20,
            SystemResourceRecord::Memory(SystemMemoryRecord {
                total_bytes: Some(2048),
                available_bytes: Some(256),
            }),
        ));

        let snapshot = registry.get_snapshot();
        let (timestamp, memory) = snapshot.last_memory_record.unwrap();
        assert_eq!(timestamp, Duration::from_nanos(20));
        assert_eq!(memory.total_bytes, Some(2048));
        assert!(snapshot.last_cpu_record.is_none());
    }

    #[test]
    fn clones_share_collection_state() {
        let registry = SystemRegistry::new();
        let registry_clone = registry.clone();

        registry_clone.update(&record(
            10,
            SystemResourceRecord::Memory(SystemMemoryRecord {
                total_bytes: Some(1024),
                available_bytes: Some(512),
            }),
        ));

        let snapshot = registry.get_snapshot();
        let (_, memory) = snapshot.last_memory_record.unwrap();
        assert_eq!(memory.available_bytes, Some(512));
    }
}
