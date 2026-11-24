//! Observation record encoding.
//!
//! [`proto`] contains the observation messages generated from the shared schema.
//! [`encode_process`] maps a process observation to a generated message and
//! serializes it using `prost`. It preserves identities, sampling timestamps,
//! thread entries, and the distinction between unavailable measurements and zero.
//! The delivery protocol defines record framing and batching.

use crate::output_model as model;
use prost::Message;

/// Protobuf messages generated from the `rdk.observer.v1` schema.
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/rdk.observer.v1.rs"));
}

/// Encodes one process observation as an unframed Protobuf message.
pub fn encode_process(record: model::ObservationRecord) -> Vec<u8> {
    process_message(record).encode_to_vec()
}

fn process_message(record: model::ObservationRecord) -> proto::ProcessRecord {
    use model::ResourceRecord;
    use proto::process_record::Resource;

    let resource = match record.resource {
        ResourceRecord::Memory(record) => Resource::Memory(memory_message(record)),
        ResourceRecord::Cpu(record) => Resource::Cpu(cpu_message(record)),
        ResourceRecord::Storage(record) => Resource::Storage(storage_message(record)),
        ResourceRecord::Threads(records) => Resource::Threads(proto::ThreadRecords {
            entries: records.into_iter().map(thread_message).collect(),
        }),
    };

    let identity = record.process_identity;
    proto::ProcessRecord {
        process_identity: Some(proto::ProcessIdentity {
            pid: identity.pid,
            start_time_ticks: identity.start_time_ticks,
            logical_label: identity.logical_label,
            security_label: identity
                .security_label
                .map(|label| proto::ProcessSecurityLabel {
                    module: label.module,
                    value: label.value,
                }),
        }),
        timestamp_unix_ns: record.timestamp_unix_ns,
        timestamp_monotonic_ns: record.timestamp_monotonic_ns,
        process_state: None,
        resource: Some(resource),
    }
}

fn memory_message(record: model::MemoryRecord) -> proto::MemoryRecord {
    proto::MemoryRecord {
        virtual_bytes: record.virtual_bytes,
        resident_bytes: record.resident_bytes,
        proportional_bytes: record.proportional_bytes,
        forensic: None,
    }
}

fn cpu_message(record: model::CpuRecord) -> proto::CpuRecord {
    proto::CpuRecord {
        clock_ticks_per_second: record.clock_ticks_per_second,
        counters: Some(proto::CpuCounters {
            user_ticks: record.counters.user_ticks,
            system_ticks: record.counters.system_ticks,
        }),
        utilization: record.utilization.map(|utilization| proto::CpuUtilization {
            interval_ns: utilization.interval_ns,
            percent: utilization.percent,
        }),
        forensic: None,
    }
}

fn storage_message(record: model::StorageRecord) -> proto::StorageRecord {
    proto::StorageRecord {
        counters: Some(proto::StorageCounters {
            read_bytes: record.counters.read_bytes,
            write_bytes: record.counters.write_bytes,
            read_syscalls: record.counters.read_syscalls,
            write_syscalls: record.counters.write_syscalls,
        }),
        rates: record.rates.map(|rates| proto::IoRates {
            interval_ns: rates.interval_ns,
            read_bytes_per_second: rates.read_bytes_per_second,
            write_bytes_per_second: rates.write_bytes_per_second,
        }),
    }
}

fn thread_message(record: model::ThreadRecord) -> proto::ThreadRecord {
    proto::ThreadRecord {
        identity: Some(proto::ThreadIdentity {
            tid: record.identity.tid,
            start_time_ticks: record.identity.start_time_ticks,
        }),
        name: record.name,
        state: record.state.map(|state| state.to_string()),
        priority: record.priority,
        nice: record.nice,
        last_cpu: record.last_cpu,
        voluntary_context_switches: record.voluntary_context_switches,
        nonvoluntary_context_switches: record.nonvoluntary_context_switches,
        cpu: record.cpu.map(cpu_message),
        storage: record.storage.map(storage_message),
        memory: None,
        network: None,
    }
}
