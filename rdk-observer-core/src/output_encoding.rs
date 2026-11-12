//! Observation record encoding.
//!
//! Maps [`crate::output_model`] records to generated [`proto`] messages and back.
//! Encoding preserves identities, timestamps, optional measurements, and empty
//! resource collections. Decoding rejects missing application-required fields
//! rather than inventing identities or measurements from Protobuf defaults.
//! Conversions are grouped by resource to match the shared schema. Shared
//! forensic CPU, memory, and TCP/UDP records stay independent of normal
//! measurements, including absent counters and measured zero values.
//!
//! Each encode/decode function handles one raw Protobuf message, without a length
//! prefix. The caller must know whether it is a process or system record; message
//! framing, batching, and routing belong to the delivery protocol. Unknown fields
//! are ignored when decoding and are not retained when re-encoding the model.

use crate::output_model as model;
use prost::Message;
use std::fmt;

/// Generated messages from `proto/observation.proto`, package `rdk.observer.v1`.
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/rdk.observer.v1.rs"));
}

#[derive(Debug)]
pub enum DecodeError {
    Protobuf(prost::DecodeError),
    MissingField(&'static str),
    /// A process state must contain exactly one Unicode scalar value.
    InvalidProcessState(String),
    /// A thread state must contain exactly one Unicode scalar value.
    InvalidThreadState(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protobuf(error) => write!(f, "invalid Protobuf: {error}"),
            Self::MissingField(field) => write!(f, "missing required field: {field}"),
            Self::InvalidProcessState(state) => {
                write!(
                    f,
                    "process state must contain exactly one character: {state:?}"
                )
            }
            Self::InvalidThreadState(state) => {
                write!(
                    f,
                    "thread state must contain exactly one character: {state:?}"
                )
            }
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Protobuf(error) => Some(error),
            _ => None,
        }
    }
}

impl From<prost::DecodeError> for DecodeError {
    fn from(error: prost::DecodeError) -> Self {
        Self::Protobuf(error)
    }
}

/// Encodes one process observation without consuming it or adding framing.
/// Callers must supply records that satisfy the schema's application requirements.
pub fn encode_process_record(record: &model::ProcessRecord) -> Vec<u8> {
    proto::ProcessRecord::from(record).encode_to_vec()
}

/// Decodes one complete process observation and validates its required fields.
pub fn decode_process_record(bytes: &[u8]) -> Result<model::ProcessRecord, DecodeError> {
    proto::ProcessRecord::decode(bytes)?.try_into()
}

/// Encodes one system observation without consuming it or adding framing.
pub fn encode_system_record(record: &model::SystemRecord) -> Vec<u8> {
    proto::SystemRecord::from(record).encode_to_vec()
}

/// Decodes one complete system observation and validates its required fields.
pub fn decode_system_record(bytes: &[u8]) -> Result<model::SystemRecord, DecodeError> {
    proto::SystemRecord::decode(bytes)?.try_into()
}

fn required<T>(value: Option<T>, field: &'static str) -> Result<T, DecodeError> {
    value.ok_or(DecodeError::MissingField(field))
}

fn decode_state(
    value: Option<String>,
    invalid_state: fn(String) -> DecodeError,
) -> Result<Option<char>, DecodeError> {
    value
        .map(|state| {
            let mut chars = state.chars();
            match (chars.next(), chars.next()) {
                (Some(character), None) => Ok(character),
                _ => Err(invalid_state(state)),
            }
        })
        .transpose()
}

// These records contain only Copy scalars with identical wire/model types.
macro_rules! field_copy_conversion {
    ($name:ident { $($field:ident),+ $(,)? }) => {
        impl From<&model::$name> for proto::$name {
            fn from(value: &model::$name) -> Self {
                Self { $($field: value.$field),+ }
            }
        }

        impl From<proto::$name> for model::$name {
            fn from(value: proto::$name) -> Self {
                Self { $($field: value.$field),+ }
            }
        }
    };
}

// -----------------------------------------------------------------------------
// Observation envelopes and process identity
// -----------------------------------------------------------------------------

impl From<&model::ProcessIdentity> for proto::ProcessIdentity {
    fn from(value: &model::ProcessIdentity) -> Self {
        Self {
            pid: value.pid,
            start_time_ticks: value.start_time_ticks,
            logical_label: value.logical_label.clone(),
            security_label: value.security_label.as_ref().map(Into::into),
        }
    }
}

impl From<proto::ProcessIdentity> for model::ProcessIdentity {
    fn from(value: proto::ProcessIdentity) -> Self {
        Self {
            pid: value.pid,
            start_time_ticks: value.start_time_ticks,
            logical_label: value.logical_label,
            security_label: value.security_label.map(Into::into),
        }
    }
}

impl From<&model::ProcessSecurityLabel> for proto::ProcessSecurityLabel {
    fn from(value: &model::ProcessSecurityLabel) -> Self {
        Self {
            module: value.module.clone(),
            value: value.value.clone(),
        }
    }
}

impl From<proto::ProcessSecurityLabel> for model::ProcessSecurityLabel {
    fn from(value: proto::ProcessSecurityLabel) -> Self {
        Self {
            module: value.module,
            value: value.value,
        }
    }
}

impl From<&model::ProcessRecord> for proto::ProcessRecord {
    fn from(value: &model::ProcessRecord) -> Self {
        Self {
            process_identity: Some((&value.process_identity).into()),
            timestamp_unix_ns: value.timestamp_unix_ns,
            timestamp_monotonic_ns: value.timestamp_monotonic_ns,
            process_state: value.process_state.map(|state| state.to_string()),
            resource: Some((&value.resource).into()),
        }
    }
}

impl TryFrom<proto::ProcessRecord> for model::ProcessRecord {
    type Error = DecodeError;

    fn try_from(value: proto::ProcessRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            process_identity: required(value.process_identity, "ProcessRecord.process_identity")?
                .into(),
            timestamp_unix_ns: value.timestamp_unix_ns,
            timestamp_monotonic_ns: value.timestamp_monotonic_ns,
            process_state: decode_state(value.process_state, DecodeError::InvalidProcessState)?,
            resource: required(value.resource, "ProcessRecord.resource")?.try_into()?,
        })
    }
}

impl From<&model::SystemRecord> for proto::SystemRecord {
    fn from(value: &model::SystemRecord) -> Self {
        Self {
            timestamp_unix_ns: value.timestamp_unix_ns,
            timestamp_monotonic_ns: value.timestamp_monotonic_ns,
            resource: Some((&value.resource).into()),
        }
    }
}

impl TryFrom<proto::SystemRecord> for model::SystemRecord {
    type Error = DecodeError;

    fn try_from(value: proto::SystemRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            timestamp_unix_ns: value.timestamp_unix_ns,
            timestamp_monotonic_ns: value.timestamp_monotonic_ns,
            resource: required(value.resource, "SystemRecord.resource")?.try_into()?,
        })
    }
}

impl From<&model::ResourceRecord> for proto::process_record::Resource {
    fn from(value: &model::ResourceRecord) -> Self {
        match value {
            model::ResourceRecord::Memory(record) => Self::Memory(record.into()),
            model::ResourceRecord::Cpu(record) => Self::Cpu(record.into()),
            model::ResourceRecord::Storage(record) => Self::Storage(record.into()),
            model::ResourceRecord::Network(record) => Self::Network(record.into()),
            model::ResourceRecord::Threads(records) => Self::Threads(proto::ThreadRecords {
                entries: records.iter().map(Into::into).collect(),
            }),
        }
    }
}

impl TryFrom<proto::process_record::Resource> for model::ResourceRecord {
    type Error = DecodeError;

    fn try_from(value: proto::process_record::Resource) -> Result<Self, Self::Error> {
        use proto::process_record::Resource;
        Ok(match value {
            Resource::Memory(record) => Self::Memory(record.into()),
            Resource::Cpu(record) => Self::Cpu(record.try_into()?),
            Resource::Storage(record) => Self::Storage(record.try_into()?),
            Resource::Network(record) => Self::Network(record.try_into()?),
            Resource::Threads(records) => Self::Threads(
                records
                    .entries
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            ),
        })
    }
}

impl From<&model::SystemResourceRecord> for proto::system_record::Resource {
    fn from(value: &model::SystemResourceRecord) -> Self {
        match value {
            model::SystemResourceRecord::Cpu(record) => Self::Cpu(record.into()),
            model::SystemResourceRecord::Memory(record) => Self::Memory(record.into()),
            model::SystemResourceRecord::Storage(records) => {
                Self::Storage(proto::SystemStorageRecords {
                    entries: records.iter().map(Into::into).collect(),
                })
            }
            model::SystemResourceRecord::Pressure(record) => Self::Pressure(record.into()),
            model::SystemResourceRecord::Network(records) => {
                Self::Network(proto::SystemNetworkRecords {
                    entries: records.iter().map(Into::into).collect(),
                })
            }
        }
    }
}

impl TryFrom<proto::system_record::Resource> for model::SystemResourceRecord {
    type Error = DecodeError;

    fn try_from(value: proto::system_record::Resource) -> Result<Self, Self::Error> {
        use proto::system_record::Resource;
        Ok(match value {
            Resource::Cpu(record) => Self::Cpu(record.try_into()?),
            Resource::Memory(record) => Self::Memory(record.into()),
            Resource::Storage(records) => Self::Storage(
                records
                    .entries
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            ),
            Resource::Pressure(record) => Self::Pressure(record.try_into()?),
            Resource::Network(records) => Self::Network(
                records
                    .entries
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            ),
        })
    }
}

// -----------------------------------------------------------------------------
// CPU records
// -----------------------------------------------------------------------------

impl From<&model::CpuRecord> for proto::CpuRecord {
    fn from(value: &model::CpuRecord) -> Self {
        Self {
            clock_ticks_per_second: value.clock_ticks_per_second,
            counters: value.counters.as_ref().map(Into::into),
            utilization: value.utilization.as_ref().map(Into::into),
            forensic: value.forensic.as_ref().map(Into::into),
        }
    }
}

impl TryFrom<proto::CpuRecord> for model::CpuRecord {
    type Error = DecodeError;

    fn try_from(value: proto::CpuRecord) -> Result<Self, Self::Error> {
        if value.counters.is_none() && value.forensic.is_none() {
            return Err(DecodeError::MissingField(
                "CpuRecord.counters or CpuRecord.forensic",
            ));
        }
        Ok(Self {
            clock_ticks_per_second: value.clock_ticks_per_second,
            counters: value.counters.map(Into::into),
            utilization: value.utilization.map(Into::into),
            forensic: value.forensic.map(Into::into),
        })
    }
}

field_copy_conversion!(CpuCounters {
    user_ticks,
    system_ticks
});

impl From<&model::SystemCpuRecord> for proto::SystemCpuRecord {
    fn from(value: &model::SystemCpuRecord) -> Self {
        Self {
            clock_ticks_per_second: value.clock_ticks_per_second,
            counters: Some((&value.counters).into()),
            utilization: value.utilization.as_ref().map(Into::into),
        }
    }
}

impl TryFrom<proto::SystemCpuRecord> for model::SystemCpuRecord {
    type Error = DecodeError;

    fn try_from(value: proto::SystemCpuRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            clock_ticks_per_second: value.clock_ticks_per_second,
            counters: required(value.counters, "SystemCpuRecord.counters")?.into(),
            utilization: value.utilization.map(Into::into),
        })
    }
}

field_copy_conversion!(SystemCpuCounters {
    user_ticks,
    nice_ticks,
    system_ticks,
    idle_ticks,
    iowait_ticks,
    irq_ticks,
    softirq_ticks,
    steal_ticks,
    guest_ticks,
    guest_nice_ticks,
});

field_copy_conversion!(CpuUtilization {
    interval_ns,
    percent
});

field_copy_conversion!(SystemCpuUtilization {
    interval_ns,
    percent
});

// -----------------------------------------------------------------------------
// Memory records
// -----------------------------------------------------------------------------

impl From<&model::MemoryRecord> for proto::MemoryRecord {
    fn from(value: &model::MemoryRecord) -> Self {
        Self {
            virtual_bytes: value.virtual_bytes,
            resident_bytes: value.resident_bytes,
            proportional_bytes: value.proportional_bytes,
            forensic: value.forensic.as_ref().map(Into::into),
        }
    }
}

impl From<proto::MemoryRecord> for model::MemoryRecord {
    fn from(value: proto::MemoryRecord) -> Self {
        Self {
            virtual_bytes: value.virtual_bytes,
            resident_bytes: value.resident_bytes,
            proportional_bytes: value.proportional_bytes,
            forensic: value.forensic.map(Into::into),
        }
    }
}

field_copy_conversion!(SystemMemoryRecord {
    total_bytes,
    available_bytes
});

// -----------------------------------------------------------------------------
// Storage records
// -----------------------------------------------------------------------------

impl From<&model::StorageRecord> for proto::StorageRecord {
    fn from(value: &model::StorageRecord) -> Self {
        Self {
            counters: Some((&value.counters).into()),
            rates: value.rates.as_ref().map(Into::into),
        }
    }
}

impl TryFrom<proto::StorageRecord> for model::StorageRecord {
    type Error = DecodeError;

    fn try_from(value: proto::StorageRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            counters: required(value.counters, "StorageRecord.counters")?.into(),
            rates: value.rates.map(Into::into),
        })
    }
}

field_copy_conversion!(StorageCounters {
    read_bytes,
    write_bytes,
    read_syscalls,
    write_syscalls
});

impl From<&model::SystemStorageRecord> for proto::SystemStorageRecord {
    fn from(value: &model::SystemStorageRecord) -> Self {
        Self {
            device_identity: Some((&value.device_identity).into()),
            counters: Some((&value.counters).into()),
            rates: value.rates.as_ref().map(Into::into),
        }
    }
}

impl TryFrom<proto::SystemStorageRecord> for model::SystemStorageRecord {
    type Error = DecodeError;

    fn try_from(value: proto::SystemStorageRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            device_identity: required(
                value.device_identity,
                "SystemStorageRecord.device_identity",
            )?
            .into(),
            counters: required(value.counters, "SystemStorageRecord.counters")?.into(),
            rates: value.rates.map(Into::into),
        })
    }
}

impl From<&model::SystemBlockDeviceIdentity> for proto::SystemBlockDeviceIdentity {
    fn from(value: &model::SystemBlockDeviceIdentity) -> Self {
        Self {
            major: value.major,
            minor: value.minor,
            name: value.name.clone(),
        }
    }
}

impl From<proto::SystemBlockDeviceIdentity> for model::SystemBlockDeviceIdentity {
    fn from(value: proto::SystemBlockDeviceIdentity) -> Self {
        Self {
            major: value.major,
            minor: value.minor,
            name: value.name,
        }
    }
}

field_copy_conversion!(SystemStorageCounters {
    read_operations,
    write_operations,
    read_sectors,
    write_sectors
});

field_copy_conversion!(IoRates {
    interval_ns,
    read_bytes_per_second,
    write_bytes_per_second
});

// -----------------------------------------------------------------------------
// Network records: process traffic, TCP/UDP socket accounting, and interfaces
// -----------------------------------------------------------------------------

impl From<&model::NetworkRecord> for proto::NetworkRecord {
    fn from(value: &model::NetworkRecord) -> Self {
        Self {
            counters: value.counters.as_ref().map(Into::into),
            rates: value.rates.as_ref().map(Into::into),
            forensic: value.forensic.as_ref().map(Into::into),
        }
    }
}

impl TryFrom<proto::NetworkRecord> for model::NetworkRecord {
    type Error = DecodeError;

    fn try_from(value: proto::NetworkRecord) -> Result<Self, Self::Error> {
        if value.counters.is_none() && value.forensic.is_none() {
            return Err(DecodeError::MissingField(
                "NetworkRecord.counters or NetworkRecord.forensic",
            ));
        }
        Ok(Self {
            counters: value.counters.map(Into::into),
            rates: value.rates.map(Into::into),
            forensic: value.forensic.map(Into::into),
        })
    }
}

field_copy_conversion!(NetworkCounters {
    rx_bytes,
    tx_bytes,
    rx_packets,
    tx_packets
});

field_copy_conversion!(ForensicTcpCounters {
    rx_bytes,
    tx_bytes,
    rx_messages,
    tx_messages
});

field_copy_conversion!(ForensicUdpCounters {
    rx_bytes,
    tx_bytes,
    rx_messages,
    tx_messages
});

field_copy_conversion!(NetworkRates {
    interval_ns,
    rx_bytes_per_second,
    tx_bytes_per_second
});

impl From<&model::SystemNetworkRecord> for proto::SystemNetworkRecord {
    fn from(value: &model::SystemNetworkRecord) -> Self {
        Self {
            interface_identity: Some((&value.interface_identity).into()),
            counters: Some((&value.counters).into()),
            rates: value.rates.as_ref().map(Into::into),
        }
    }
}

impl TryFrom<proto::SystemNetworkRecord> for model::SystemNetworkRecord {
    type Error = DecodeError;

    fn try_from(value: proto::SystemNetworkRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            interface_identity: required(
                value.interface_identity,
                "SystemNetworkRecord.interface_identity",
            )?
            .into(),
            counters: required(value.counters, "SystemNetworkRecord.counters")?.into(),
            rates: value.rates.map(Into::into),
        })
    }
}

impl From<&model::SystemNetworkInterfaceIdentity> for proto::SystemNetworkInterfaceIdentity {
    fn from(value: &model::SystemNetworkInterfaceIdentity) -> Self {
        Self {
            network_namespace_inode: value.network_namespace_inode,
            interface_index: value.interface_index,
            name: value.name.clone(),
        }
    }
}

impl From<proto::SystemNetworkInterfaceIdentity> for model::SystemNetworkInterfaceIdentity {
    fn from(value: proto::SystemNetworkInterfaceIdentity) -> Self {
        Self {
            network_namespace_inode: value.network_namespace_inode,
            interface_index: value.interface_index,
            name: value.name,
        }
    }
}

field_copy_conversion!(SystemNetworkCounters {
    rx_bytes,
    tx_bytes,
    rx_packets,
    tx_packets,
    rx_errors,
    tx_errors,
    rx_dropped,
    tx_dropped
});

// -----------------------------------------------------------------------------
// Thread records
// -----------------------------------------------------------------------------

field_copy_conversion!(ThreadIdentity {
    tid,
    start_time_ticks
});

impl From<&model::ThreadRecord> for proto::ThreadRecord {
    fn from(value: &model::ThreadRecord) -> Self {
        Self {
            identity: Some((&value.identity).into()),
            name: value.name.clone(),
            state: value.state.map(|state| state.to_string()),
            priority: value.priority,
            nice: value.nice,
            last_cpu: value.last_cpu,
            voluntary_context_switches: value.voluntary_context_switches,
            nonvoluntary_context_switches: value.nonvoluntary_context_switches,
            cpu: value.cpu.as_ref().map(Into::into),
            memory: value.memory.as_ref().map(Into::into),
            storage: value.storage.as_ref().map(Into::into),
            network: value.network.as_ref().map(Into::into),
        }
    }
}

impl TryFrom<proto::ThreadRecord> for model::ThreadRecord {
    type Error = DecodeError;

    fn try_from(value: proto::ThreadRecord) -> Result<Self, Self::Error> {
        let state = decode_state(value.state, DecodeError::InvalidThreadState)?;
        Ok(Self {
            identity: required(value.identity, "ThreadRecord.identity")?.into(),
            name: value.name,
            state,
            priority: value.priority,
            nice: value.nice,
            last_cpu: value.last_cpu,
            voluntary_context_switches: value.voluntary_context_switches,
            nonvoluntary_context_switches: value.nonvoluntary_context_switches,
            cpu: value.cpu.map(TryInto::try_into).transpose()?,
            memory: value.memory.map(Into::into),
            storage: value.storage.map(TryInto::try_into).transpose()?,
            network: value.network.map(TryInto::try_into).transpose()?,
        })
    }
}

// -----------------------------------------------------------------------------
// Pressure records
// -----------------------------------------------------------------------------

impl From<&model::SystemPressureRecord> for proto::SystemPressureRecord {
    fn from(value: &model::SystemPressureRecord) -> Self {
        Self {
            cpu: value.cpu.as_ref().map(Into::into),
            memory: value.memory.as_ref().map(Into::into),
            io: value.io.as_ref().map(Into::into),
        }
    }
}

impl TryFrom<proto::SystemPressureRecord> for model::SystemPressureRecord {
    type Error = DecodeError;

    fn try_from(value: proto::SystemPressureRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            cpu: value.cpu.map(TryInto::try_into).transpose()?,
            memory: value.memory.map(TryInto::try_into).transpose()?,
            io: value.io.map(TryInto::try_into).transpose()?,
        })
    }
}

impl From<&model::SystemPressureResourceRecord> for proto::SystemPressureResourceRecord {
    fn from(value: &model::SystemPressureResourceRecord) -> Self {
        Self {
            some: Some((&value.some).into()),
            full: Some((&value.full).into()),
        }
    }
}

impl TryFrom<proto::SystemPressureResourceRecord> for model::SystemPressureResourceRecord {
    type Error = DecodeError;

    fn try_from(value: proto::SystemPressureResourceRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            some: required(value.some, "SystemPressureResourceRecord.some")?.into(),
            full: required(value.full, "SystemPressureResourceRecord.full")?.into(),
        })
    }
}

field_copy_conversion!(SystemPressureStallRecord {
    avg10_percent,
    avg60_percent,
    avg300_percent,
    total_microseconds
});

// -----------------------------------------------------------------------------
// Shared forensic measurements
// -----------------------------------------------------------------------------

impl From<&model::ForensicCpuRecord> for proto::ForensicCpuRecord {
    fn from(value: &model::ForensicCpuRecord) -> Self {
        Self {
            runtime_ns: value.runtime_ns,
            utilization: value.utilization.as_ref().map(Into::into),
        }
    }
}

impl From<proto::ForensicCpuRecord> for model::ForensicCpuRecord {
    fn from(value: proto::ForensicCpuRecord) -> Self {
        Self {
            runtime_ns: value.runtime_ns,
            utilization: value.utilization.map(Into::into),
        }
    }
}

field_copy_conversion!(ForensicCpuUtilization {
    interval_ns,
    percent
});
field_copy_conversion!(ForensicMemoryRecord {
    minor_page_faults,
    major_page_faults
});

impl From<&model::ForensicNetworkRecord> for proto::ForensicNetworkRecord {
    fn from(value: &model::ForensicNetworkRecord) -> Self {
        Self {
            tcp: value.tcp.as_ref().map(Into::into),
            udp: value.udp.as_ref().map(Into::into),
            rates: value.rates.as_ref().map(Into::into),
        }
    }
}

impl From<proto::ForensicNetworkRecord> for model::ForensicNetworkRecord {
    fn from(value: proto::ForensicNetworkRecord) -> Self {
        Self {
            tcp: value.tcp.map(Into::into),
            udp: value.udp.map(Into::into),
            rates: value.rates.map(Into::into),
        }
    }
}

field_copy_conversion!(ForensicNetworkRates {
    interval_ns,
    rx_bytes_per_second,
    tx_bytes_per_second
});

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn required_accepts_present_zero_and_empty_values() {
        assert_eq!(required(Some(0), "counter").unwrap(), 0);
        assert_eq!(required(Some(String::new()), "label").unwrap(), "");
        assert!(
            required(Some(Vec::<u64>::new()), "entries")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn required_reports_the_missing_field() {
        assert!(matches!(
            required::<u64>(None, "CpuRecord.counters"),
            Err(DecodeError::MissingField("CpuRecord.counters"))
        ));
    }

    #[test]
    fn application_errors_describe_the_problem_without_an_underlying_source() {
        let missing = DecodeError::MissingField("ThreadRecord.identity");
        assert_eq!(
            missing.to_string(),
            "missing required field: ThreadRecord.identity"
        );
        assert!(missing.source().is_none());

        let invalid = DecodeError::InvalidThreadState("RS".into());
        assert_eq!(
            invalid.to_string(),
            "thread state must contain exactly one character: \"RS\""
        );
        assert!(invalid.source().is_none());

        let invalid_process = DecodeError::InvalidProcessState("RS".into());
        assert_eq!(
            invalid_process.to_string(),
            "process state must contain exactly one character: \"RS\""
        );
        assert!(invalid_process.source().is_none());
    }

    #[test]
    fn protobuf_errors_retain_their_message_and_source() {
        let source = proto::ProcessRecord::decode(&[0xff][..]).unwrap_err();
        let message = source.to_string();
        let error = DecodeError::from(source);

        assert!(matches!(&error, DecodeError::Protobuf(_)));
        assert_eq!(error.to_string(), format!("invalid Protobuf: {message}"));
        assert_eq!(error.source().unwrap().to_string(), message);
    }

    #[test]
    fn field_copy_conversion_preserves_absence_zero_and_integer_limits() {
        for user_ticks in [None, Some(0), Some(u64::MAX)] {
            for system_ticks in [None, Some(0), Some(u64::MAX)] {
                let counters = model::CpuCounters {
                    user_ticks,
                    system_ticks,
                };
                let wire = proto::CpuCounters::from(&counters);

                assert_eq!(wire.user_ticks, user_ticks);
                assert_eq!(wire.system_ticks, system_ticks);
                assert_eq!(model::CpuCounters::from(wire), counters);
            }
        }
    }

    #[test]
    fn cpu_sources_can_be_normal_forensic_or_combined_but_not_both_absent() {
        for normal in [false, true] {
            for forensic in [false, true] {
                let wire = proto::CpuRecord {
                    clock_ticks_per_second: 100,
                    counters: normal.then_some(proto::CpuCounters {
                        user_ticks: Some(0),
                        system_ticks: Some(u64::MAX),
                    }),
                    utilization: Some(proto::CpuUtilization {
                        interval_ns: 1,
                        percent: 25.0,
                    }),
                    forensic: forensic.then_some(proto::ForensicCpuRecord {
                        runtime_ns: Some(0),
                        utilization: Some(proto::ForensicCpuUtilization {
                            interval_ns: u64::MAX,
                            percent: 250.0,
                        }),
                    }),
                };
                let decoded = model::CpuRecord::try_from(wire);
                if !normal && !forensic {
                    assert!(matches!(
                        decoded,
                        Err(DecodeError::MissingField(
                            "CpuRecord.counters or CpuRecord.forensic"
                        ))
                    ));
                    continue;
                }

                let record = decoded.unwrap();
                assert_eq!(record.counters.is_some(), normal);
                assert_eq!(record.forensic.is_some(), forensic);
                assert_eq!(record.utilization.unwrap().percent, 25.0);
                let encoded = proto::CpuRecord::from(&record);
                if normal {
                    let counters = encoded.counters.unwrap();
                    assert_eq!(counters.user_ticks, Some(0));
                    assert_eq!(counters.system_ticks, Some(u64::MAX));
                }
                if forensic {
                    let capture = encoded.forensic.unwrap();
                    assert_eq!(capture.runtime_ns, Some(0));
                    assert_eq!(capture.utilization.unwrap().percent, 250.0);
                    assert_eq!(capture.utilization.unwrap().interval_ns, u64::MAX);
                }
            }
        }
    }

    #[test]
    fn network_sources_keep_normal_and_forensic_totals_and_rates_independent() {
        for normal in [false, true] {
            for forensic in [false, true] {
                let wire = proto::NetworkRecord {
                    counters: normal.then_some(proto::NetworkCounters {
                        rx_bytes: Some(11),
                        tx_bytes: Some(12),
                        rx_packets: Some(13),
                        tx_packets: None,
                    }),
                    rates: Some(proto::NetworkRates {
                        interval_ns: 1,
                        rx_bytes_per_second: Some(25.0),
                        tx_bytes_per_second: None,
                    }),
                    forensic: forensic.then_some(proto::ForensicNetworkRecord {
                        tcp: Some(proto::ForensicTcpCounters {
                            rx_bytes: Some(u64::MAX),
                            tx_bytes: Some(0),
                            rx_messages: None,
                            tx_messages: Some(2),
                        }),
                        udp: Some(proto::ForensicUdpCounters {
                            rx_bytes: Some(0),
                            tx_bytes: None,
                            rx_messages: Some(3),
                            tx_messages: Some(4),
                        }),
                        rates: Some(proto::ForensicNetworkRates {
                            interval_ns: u64::MAX,
                            rx_bytes_per_second: Some(0.0),
                            tx_bytes_per_second: Some(250.0),
                        }),
                    }),
                };
                let decoded = model::NetworkRecord::try_from(wire);
                if !normal && !forensic {
                    assert!(matches!(
                        decoded,
                        Err(DecodeError::MissingField(
                            "NetworkRecord.counters or NetworkRecord.forensic"
                        ))
                    ));
                    continue;
                }

                let record = decoded.unwrap();
                assert_eq!(record.counters.is_some(), normal);
                assert_eq!(record.forensic.is_some(), forensic);
                assert_eq!(record.rates.unwrap().rx_bytes_per_second, Some(25.0));
                let encoded = proto::NetworkRecord::from(&record);
                if normal {
                    let counters = encoded.counters.unwrap();
                    assert_eq!(counters.rx_bytes, Some(11));
                    assert_eq!(counters.rx_packets, Some(13));
                    assert_eq!(counters.tx_packets, None);
                }
                if forensic {
                    let capture = encoded.forensic.unwrap();
                    assert_eq!(capture.tcp.unwrap().rx_bytes, Some(u64::MAX));
                    assert_eq!(capture.tcp.unwrap().tx_bytes, Some(0));
                    assert_eq!(capture.udp.unwrap().rx_messages, Some(3));
                    assert_eq!(capture.rates.unwrap().rx_bytes_per_second, Some(0.0));
                    assert_eq!(capture.rates.unwrap().tx_bytes_per_second, Some(250.0));
                }
            }
        }
    }

    #[test]
    fn encoding_process_identity_preserves_labels_without_consuming_them() {
        let identity = model::ProcessIdentity {
            pid: 42,
            start_time_ticks: 123,
            logical_label: Some("player".into()),
            security_label: Some(model::ProcessSecurityLabel {
                module: "selinux".into(),
                value: "system_u:system_r:player_t:s0".into(),
            }),
        };
        let mut wire = proto::ProcessIdentity::from(&identity);

        assert_eq!(wire.pid, 42);
        assert_eq!(wire.start_time_ticks, 123);
        assert_eq!(model::ProcessIdentity::from(wire.clone()), identity);

        wire.logical_label.as_mut().unwrap().clear();
        wire.security_label.as_mut().unwrap().value.clear();
        assert_eq!(identity.logical_label.as_deref(), Some("player"));
        assert_eq!(
            identity.security_label.as_ref().unwrap().value,
            "system_u:system_r:player_t:s0"
        );
    }

    #[test]
    fn process_state_conversion_accepts_absence_and_single_unicode_scalars() {
        for state in [
            None,
            Some('R'),
            Some('S'),
            Some('Z'),
            Some('é'),
            Some('🦀'),
            Some('\0'),
            Some('\u{301}'),
        ] {
            let wire = proto::ProcessRecord {
                process_identity: Some(Default::default()),
                process_state: state.map(|character| character.to_string()),
                resource: Some(proto::process_record::Resource::Memory(Default::default())),
                ..Default::default()
            };
            let record = model::ProcessRecord::try_from(wire.clone()).unwrap();

            assert_eq!(record.process_state, state);
            assert_eq!(proto::ProcessRecord::from(&record), wire);
        }
    }

    #[test]
    fn process_state_decoding_rejects_empty_and_multiple_unicode_scalars() {
        for state in ["", "RS", "e\u{301}", "🇳🇱"] {
            let wire = proto::ProcessRecord {
                process_identity: Some(Default::default()),
                process_state: Some(state.into()),
                resource: Some(proto::process_record::Resource::Memory(Default::default())),
                ..Default::default()
            };

            assert!(matches!(
                decode_process_record(&wire.encode_to_vec()),
                Err(DecodeError::InvalidProcessState(actual)) if actual == state
            ));
        }
    }

    #[test]
    fn thread_state_conversion_accepts_absence_and_single_unicode_scalars() {
        for state in [
            None,
            Some('R'),
            Some('é'),
            Some('🦀'),
            Some('\0'),
            Some('\u{301}'),
        ] {
            let wire = proto::ThreadRecord {
                identity: Some(proto::ThreadIdentity {
                    tid: 7,
                    start_time_ticks: 12,
                }),
                state: state.map(|character| character.to_string()),
                ..Default::default()
            };
            let record = model::ThreadRecord::try_from(wire.clone()).unwrap();

            assert_eq!(record.state, state);
            assert_eq!(record.identity.tid, 7);
            assert_eq!(record.identity.start_time_ticks, 12);
            assert_eq!(proto::ThreadRecord::from(&record), wire);
        }
    }

    #[test]
    fn thread_state_conversion_rejects_empty_and_multiple_unicode_scalars() {
        for state in ["", "RS", "e\u{301}", "🇳🇱"] {
            let wire = proto::ThreadRecord {
                identity: Some(Default::default()),
                state: Some(state.into()),
                ..Default::default()
            };

            assert!(matches!(
                model::ThreadRecord::try_from(wire),
                Err(DecodeError::InvalidThreadState(actual)) if actual == state
            ));
        }
    }

    #[test]
    fn invalid_later_thread_entry_rejects_the_whole_collection() {
        let wire = proto::process_record::Resource::Threads(proto::ThreadRecords {
            entries: vec![
                proto::ThreadRecord {
                    identity: Some(Default::default()),
                    ..Default::default()
                },
                proto::ThreadRecord {
                    identity: Some(Default::default()),
                    storage: Some(Default::default()),
                    ..Default::default()
                },
            ],
        });

        assert!(matches!(
            model::ResourceRecord::try_from(wire),
            Err(DecodeError::MissingField("StorageRecord.counters"))
        ));
    }

    #[test]
    fn invalid_later_system_entry_rejects_the_whole_collection() {
        use proto::system_record::Resource;

        for (wire, field) in [
            (
                Resource::Storage(proto::SystemStorageRecords {
                    entries: vec![
                        proto::SystemStorageRecord {
                            device_identity: Some(Default::default()),
                            counters: Some(Default::default()),
                            rates: None,
                        },
                        proto::SystemStorageRecord {
                            device_identity: Some(Default::default()),
                            ..Default::default()
                        },
                    ],
                }),
                "SystemStorageRecord.counters",
            ),
            (
                Resource::Network(proto::SystemNetworkRecords {
                    entries: vec![
                        proto::SystemNetworkRecord {
                            interface_identity: Some(Default::default()),
                            counters: Some(Default::default()),
                            rates: None,
                        },
                        proto::SystemNetworkRecord {
                            interface_identity: Some(Default::default()),
                            ..Default::default()
                        },
                    ],
                }),
                "SystemNetworkRecord.counters",
            ),
        ] {
            assert!(matches!(
                model::SystemResourceRecord::try_from(wire),
                Err(DecodeError::MissingField(actual)) if actual == field
            ));
        }
    }

    #[test]
    fn optional_rates_preserve_negative_zero_and_non_finite_values() {
        for rate in [-0.0_f64, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let wire = proto::StorageRecord {
                counters: Some(Default::default()),
                rates: Some(proto::IoRates {
                    interval_ns: 1_000_000_000,
                    read_bytes_per_second: Some(rate),
                    write_bytes_per_second: None,
                }),
            };
            let record = model::StorageRecord::try_from(wire).unwrap();
            let bytes = proto::StorageRecord::from(&record).encode_to_vec();
            let decoded = proto::StorageRecord::decode(bytes.as_slice()).unwrap();
            let rates = decoded.rates.unwrap();

            assert_eq!(
                rates.read_bytes_per_second.unwrap().to_bits(),
                rate.to_bits()
            );
            assert_eq!(rates.write_bytes_per_second, None);
        }
    }
}
