//! Defines the observation records prepared for output encoding.
//!
//! Process observations identify one process and carry sampling timestamps with
//! memory, CPU, storage I/O, network traffic, or a collection of thread records.
//! Thread entries include identities, metadata, shared CPU/I/O/network records,
//! and optional forensic memory activity. Normal and forensic measurements stay
//! separate; the enclosing process or thread defines their scope.
//! Process identities include optional logical and security labels. System
//! observations carry their own timestamps with aggregate CPU, memory, pressure,
//! per-interface network traffic, or per-device storage I/O measurements.
//! Unavailable measurements use `None` rather than zero, and independently
//! sampled records are not an atomic snapshot.
//!
//! These types describe the data passed to `output_encoding`, which maps
//! records to shared Protobuf messages and serializes them using `prost`.

// -----------------------------------------------------------------------------
// System record
// -----------------------------------------------------------------------------

/// A timestamped resource observation for the system as a whole.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemRecord {
    /// Sampling time in nanoseconds since the Unix epoch.
    pub timestamp_unix_ns: u64,
    /// Sampling time in nanoseconds on the monotonic clock.
    pub timestamp_monotonic_ns: u64,
    /// System resource measurements captured by this observation.
    pub resource: SystemResourceRecord,
}

/// System resource measurements for CPU, memory, pressure, storage I/O, or network traffic.
#[derive(Debug, Clone, PartialEq)]
pub enum SystemResourceRecord {
    /// CPU time counters aggregated across all CPUs.
    Cpu(SystemCpuRecord),
    /// System physical memory totals and availability.
    Memory(SystemMemoryRecord),
    /// Storage I/O measurements for selected block devices; avoid counting disks and partitions twice.
    Storage(Vec<SystemStorageRecord>),
    /// System-wide CPU, memory, and I/O pressure stall information.
    Pressure(SystemPressureRecord),
    /// Traffic counters for selected network interfaces.
    Network(Vec<SystemNetworkRecord>),
}

/// Aggregate CPU counters from `/proc/stat` with optional derived utilization.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemCpuRecord {
    /// Number of clock ticks per second, used to convert CPU counters to seconds.
    pub clock_ticks_per_second: u64,
    /// Raw CPU time counters summed across all CPUs.
    pub counters: SystemCpuCounters,
    /// Aggregate CPU usage percentage, where 100% means all CPU capacity is busy.
    /// `None` means no valid baseline exists, such as on the first sample or after a reset.
    pub utilization: Option<SystemCpuUtilization>,
}

/// Aggregate CPU utilization, where 100% means all CPU capacity is busy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SystemCpuUtilization {
    pub interval_ns: u64,
    pub percent: f64,
}

/// Raw aggregate CPU time counters in clock ticks; guest time overlaps user and nice time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemCpuCounters {
    /// Cumulative time in user mode, including guest time, when available.
    pub user_ticks: Option<u64>,
    /// Cumulative time in user mode with a nice adjustment, including nice guest time.
    pub nice_ticks: Option<u64>,
    /// Cumulative time executing in kernel mode, when available.
    pub system_ticks: Option<u64>,
    /// Cumulative idle time excluding I/O wait, when available.
    pub idle_ticks: Option<u64>,
    /// Reported I/O wait time, when available; the kernel may decrease this counter.
    pub iowait_ticks: Option<u64>,
    /// Cumulative time servicing hardware interrupts, when available.
    pub irq_ticks: Option<u64>,
    /// Cumulative time servicing software interrupts, when available.
    pub softirq_ticks: Option<u64>,
    /// Cumulative time unavailable to a virtual CPU while the hypervisor runs other work.
    pub steal_ticks: Option<u64>,
    /// Cumulative guest CPU time already included in `user_ticks`, when available.
    pub guest_ticks: Option<u64>,
    /// Cumulative nice-adjusted guest CPU time already included in `nice_ticks`.
    pub guest_nice_ticks: Option<u64>,
}

/// Physical memory capacity and availability from `/proc/meminfo`, converted to bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemMemoryRecord {
    /// Total usable physical RAM in bytes (`MemTotal`), when available.
    pub total_bytes: Option<u64>,
    /// Estimated RAM available for new applications without swapping (`MemAvailable`), in bytes.
    pub available_bytes: Option<u64>,
}

/// Storage I/O counters from `/proc/diskstats` and optional rates for one block device.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemStorageRecord {
    /// Major/minor numbers and name of the measured block device.
    pub device_identity: SystemBlockDeviceIdentity,
    /// Cumulative completed block I/O operations and sector counts, distinct from process syscalls.
    pub counters: SystemStorageCounters,
    /// Byte rates derived from two valid samples of the same device, using 512 bytes per sector.
    /// `None` means rates are unavailable, such as on the first sample, after a
    /// counter reset, or when the device disappears or changes identity; zero means no I/O.
    pub rates: Option<IoRates>,
}

/// Identifies a block device by its major/minor device numbers and kernel name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SystemBlockDeviceIdentity {
    /// Major device number identifying the device driver or class.
    pub major: u32,
    /// Minor device number identifying the device or partition within that class.
    pub minor: u32,
    /// Kernel device name, such as `sda` or `mmcblk0`.
    pub name: String,
}

/// Raw cumulative block-device read/write counters; sectors always represent 512 bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemStorageCounters {
    /// Number of successfully completed block read operations, when available.
    pub read_operations: Option<u64>,
    /// Number of successfully completed block write operations, when available.
    pub write_operations: Option<u64>,
    /// Number of 512-byte sectors successfully read, when available.
    pub read_sectors: Option<u64>,
    /// Number of 512-byte sectors successfully written, when available.
    pub write_sectors: Option<u64>,
}

/// System-wide pressure stall information from `/proc/pressure`.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemPressureRecord {
    /// CPU pressure, when `/proc/pressure/cpu` is available.
    pub cpu: Option<SystemPressureResourceRecord>,
    /// Memory pressure, when `/proc/pressure/memory` is available.
    pub memory: Option<SystemPressureResourceRecord>,
    /// I/O pressure, when `/proc/pressure/io` is available.
    pub io: Option<SystemPressureResourceRecord>,
}

/// Pressure stall information for one resource.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemPressureResourceRecord {
    /// Pressure when at least some tasks were stalled.
    pub some: SystemPressureStallRecord,
    /// Pressure when all non-idle tasks were stalled.
    pub full: SystemPressureStallRecord,
}

/// Pressure stall values for one `some` or `full` record.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemPressureStallRecord {
    /// Percentage of stalled time over the last 10 seconds.
    pub avg10_percent: f32,
    /// Percentage of stalled time over the last 60 seconds.
    pub avg60_percent: f32,
    /// Percentage of stalled time over the last 300 seconds.
    pub avg300_percent: f32,
    /// Cumulative stalled time in microseconds.
    pub total_microseconds: u64,
}

// -----------------------------------------------------------------------------
// Process record
// -----------------------------------------------------------------------------

/// A timestamped resource observation associated with a process.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessRecord {
    /// Identity of the process to which this observation belongs.
    pub process_identity: ProcessIdentity,
    /// Sampling time in nanoseconds since the Unix epoch.
    pub timestamp_unix_ns: u64,
    /// Sampling time in nanoseconds on the monotonic clock.
    pub timestamp_monotonic_ns: u64,
    /// State reported by `/proc/<pid>/stat` for the process's main thread.
    /// Sampled alongside CPU counters; `None` means this observation did not sample it.
    pub process_state: Option<char>,
    /// Resource measurements captured by this observation.
    pub resource: ResourceRecord,
}

/// Resource measurements for memory, CPU, storage I/O, network traffic, or threads.
#[derive(Debug, Clone, PartialEq)]
pub enum ResourceRecord {
    Memory(MemoryRecord),
    Cpu(CpuRecord),
    Storage(StorageRecord),
    Threads(Vec<ThreadRecord>),
    Network(NetworkRecord),
}

// -----------------------------------------------------------------------------
// Process identification
// -----------------------------------------------------------------------------

/// Identifies a process by PID and start time, with optional descriptive labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    /// Operating system process ID.
    pub pid: u32,
    /// Process start time in clock ticks since system boot, used to detect PID reuse.
    pub start_time_ticks: u64,
    /// Optional logical name used to describe or group the process.
    pub logical_label: Option<String>,
    /// Optional security context assigned to the process.
    pub security_label: Option<ProcessSecurityLabel>,
}

/// A process security label paired with the security module that supplies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSecurityLabel {
    /// Name of the security module that supplies the label.
    pub module: String,
    /// Security label text reported by the module.
    pub value: String,
}

// -----------------------------------------------------------------------------
// CPU record
// -----------------------------------------------------------------------------

/// Shared process/thread CPU measurements from normal collection and forensic capture.
/// At least one of `counters` or `forensic` must be present when decoding.
#[derive(Debug, Clone, PartialEq)]
pub struct CpuRecord {
    /// Number of clock ticks per second, used to convert CPU counters to seconds.
    pub clock_ticks_per_second: u64,
    /// Optional cumulative procfs counters, independent of forensic capture.
    pub counters: Option<CpuCounters>,
    /// Optional CPU usage percentage derived from consecutive samples.
    pub utilization: Option<CpuUtilization>,
    /// Optional eBPF scheduler capture, with nanosecond runtime and its own utilization.
    pub forensic: Option<ForensicCpuRecord>,
}

/// Cumulative CPU time spent in user and system modes, measured in clock ticks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CpuCounters {
    /// Cumulative CPU time spent in user mode, in clock ticks, when available.
    pub user_ticks: Option<u64>,
    /// Cumulative CPU time spent in kernel mode, in clock ticks, when available.
    pub system_ticks: Option<u64>,
}

/// CPU utilization as a percentage over a sampling interval in nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuUtilization {
    /// Elapsed time between the samples used to calculate utilization, in nanoseconds.
    pub interval_ns: u64,
    /// CPU usage over the sampling interval, expressed as a percentage.
    pub percent: f64,
}

// -----------------------------------------------------------------------------
// Memory record
// -----------------------------------------------------------------------------

/// Optional virtual, resident, and proportional memory measurements in bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryRecord {
    /// Total virtual address space size in bytes, when available.
    pub virtual_bytes: Option<u64>,
    /// Memory currently resident in physical RAM in bytes, when available.
    pub resident_bytes: Option<u64>,
    /// Resident memory in bytes with shared pages divided among their users, when available.
    pub proportional_bytes: Option<u64>,
    /// Optional perf fault counters, independent of process address-space sizes.
    pub forensic: Option<ForensicMemoryRecord>,
}

// -----------------------------------------------------------------------------
// I/O record
// -----------------------------------------------------------------------------

/// Cumulative storage I/O counters and optional transfer rates.
#[derive(Debug, Clone, PartialEq)]
pub struct StorageRecord {
    /// Cumulative storage I/O byte and syscall counts.
    pub counters: StorageCounters,
    /// Read and write rates derived from two valid consecutive samples.
    /// `None` means rates are unavailable, such as on the first sample, after a
    /// counter reset, or when the process identity changes; counters remain usable.
    /// A rate of zero means no I/O occurred during the interval.
    pub rates: Option<IoRates>,
}

/// Cumulative storage I/O byte counts and read and write syscall counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageCounters {
    /// Cumulative number of bytes read from storage, when available.
    pub read_bytes: Option<u64>,
    /// Cumulative number of bytes written to storage, when available.
    pub write_bytes: Option<u64>,
    /// Cumulative number of read I/O system calls, when available.
    pub read_syscalls: Option<u64>,
    /// Cumulative number of write I/O system calls, when available.
    pub write_syscalls: Option<u64>,
}

/// Read and write byte rates over a sampling interval in nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IoRates {
    /// Elapsed time between the samples used to calculate rates, in nanoseconds.
    pub interval_ns: u64,
    /// Average bytes read per second over the interval, when available.
    pub read_bytes_per_second: Option<f64>,
    /// Average bytes written per second over the interval, when available.
    pub write_bytes_per_second: Option<f64>,
}

// -----------------------------------------------------------------------------
// Network record
// -----------------------------------------------------------------------------

/// Shared traffic measurements attributed to the enclosing process or thread.
/// At least one of `counters` or `forensic` must be present when decoding.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkRecord {
    /// Cumulative traffic measurements over the source's accounting lifetime.
    pub counters: Option<NetworkCounters>,
    /// Byte rates from two valid samples of the same source and enclosing identity.
    /// First samples, counter resets, and identity changes invalidate the baseline.
    pub rates: Option<NetworkRates>,
    /// Optional eBPF socket accounting, separate from normal traffic totals and rates.
    pub forensic: Option<ForensicNetworkRecord>,
}

/// Normal traffic counters for a process or thread over the source's lifetime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkCounters {
    pub rx_bytes: Option<u64>,
    pub tx_bytes: Option<u64>,
    /// Packet counts require a source that provides them.
    pub rx_packets: Option<u64>,
    pub tx_packets: Option<u64>,
}

/// Scheduled time during the current eBPF capture, shared by process and thread records.
/// Capture restarts reset the counter; procfs tick counters use an independent baseline.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ForensicCpuRecord {
    pub runtime_ns: Option<u64>,
    pub utilization: Option<ForensicCpuUtilization>,
}

/// CPU usage derived from capture runtime; 100% represents one fully occupied CPU.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForensicCpuUtilization {
    pub interval_ns: u64,
    pub percent: f64,
}

/// Perf page-fault activity during capture; these counts do not measure memory sizes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ForensicMemoryRecord {
    pub minor_page_faults: Option<u64>,
    pub major_page_faults: Option<u64>,
}

/// Socket payload accounting during capture, shared by process and thread records.
/// Process observations aggregate the target thread group; thread observations
/// describe one thread. Capture counters remain independent of normal totals.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ForensicNetworkRecord {
    pub tcp: Option<ForensicTcpCounters>,
    pub udp: Option<ForensicUdpCounters>,
    pub rates: Option<ForensicNetworkRates>,
}

/// Byte rates derived from captured TCP/UDP totals within the same capture and identity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForensicNetworkRates {
    pub interval_ns: u64,
    pub rx_bytes_per_second: Option<f64>,
    pub tx_bytes_per_second: Option<f64>,
}

/// Cumulative TCP socket accounting over the capture lifetime.
/// RX/TX are relative to the enclosing process or thread. Count each completed operation
/// with a positive return value once, using the actual returned byte count for
/// partial transfers. Exclude errors, zero-byte results, MSG_PEEK, and MSG_ERRQUEUE.
/// Payload bytes exclude headers and are not counted again for retransmissions.
/// Messages mean successful socket operations, not packets or application frames.
/// Unavailable counters use `None`; measured zero means no traffic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForensicTcpCounters {
    pub rx_bytes: Option<u64>,
    pub tx_bytes: Option<u64>,
    pub rx_messages: Option<u64>,
    pub tx_messages: Option<u64>,
}

/// Cumulative UDP socket accounting over the capture lifetime.
/// RX/TX are relative to the enclosing process or thread. Bytes count payload copied to the
/// application on receive or accepted by the kernel on send, excluding headers
/// and discarded bytes from truncated receives. Messages count successful
/// observed datagram operations, including zero-byte ones. Exclude errors,
/// MSG_PEEK, and MSG_ERRQUEUE; do not infer wire datagram counts from segmentation.
/// Unavailable counters use `None`; measured zero means no traffic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForensicUdpCounters {
    pub rx_bytes: Option<u64>,
    pub tx_bytes: Option<u64>,
    pub rx_messages: Option<u64>,
    pub tx_messages: Option<u64>,
}

/// Receive and transmit byte rates over a sampling interval in nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NetworkRates {
    pub interval_ns: u64,
    pub rx_bytes_per_second: Option<f64>,
    pub tx_bytes_per_second: Option<f64>,
}

/// Traffic counters for one network interface, separate from process traffic.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemNetworkRecord {
    pub interface_identity: SystemNetworkInterfaceIdentity,
    pub counters: SystemNetworkCounters,
    /// Disappearance, recreation, and counter resets invalidate rate baselines.
    pub rates: Option<NetworkRates>,
}

/// Identifies an interface within the observed host, boot, and network namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SystemNetworkInterfaceIdentity {
    pub network_namespace_inode: u64,
    pub interface_index: u32,
    pub name: String,
}

/// Cumulative interface statistics, distinct from TCP/UDP socket accounting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemNetworkCounters {
    pub rx_bytes: Option<u64>,
    pub tx_bytes: Option<u64>,
    pub rx_packets: Option<u64>,
    pub tx_packets: Option<u64>,
    pub rx_errors: Option<u64>,
    pub tx_errors: Option<u64>,
    pub rx_dropped: Option<u64>,
    pub tx_dropped: Option<u64>,
}

// -----------------------------------------------------------------------------
// Thread record
// -----------------------------------------------------------------------------

/// Identifies a thread by its thread ID and start time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadIdentity {
    /// Operating system thread ID.
    pub tid: u32,
    /// Thread start time in clock ticks since system boot, used to detect TID reuse.
    pub start_time_ticks: u64,
}

/// A thread's identity, metadata, shared normal measurements, and forensic activity.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadRecord {
    /// Identity of the thread to which these measurements belong.
    pub identity: ThreadIdentity,
    /// Thread name reported by the operating system, when available.
    pub name: Option<String>,
    /// Operating system state code, such as 'R' for running or 'S' for sleeping.
    pub state: Option<char>,
    /// Raw scheduling priority reported by the operating system, when available.
    pub priority: Option<i64>,
    /// Nice value influencing scheduling priority; lower values favor the thread.
    pub nice: Option<i32>,
    /// Index of the CPU on which the thread last executed, when available.
    pub last_cpu: Option<u32>,
    /// Cumulative context switches caused by the thread yielding or blocking.
    pub voluntary_context_switches: Option<u64>,
    /// Cumulative context switches caused by the scheduler preempting the thread.
    pub nonvoluntary_context_switches: Option<u64>,
    /// Optional CPU time counters and utilization for this thread.
    pub cpu: Option<CpuRecord>,
    /// Optional page-fault activity attributed to this thread during capture.
    pub memory: Option<ForensicMemoryRecord>,
    /// Optional storage I/O counters and transfer rates for this thread.
    pub storage: Option<StorageRecord>,
    /// Optional normal and forensic network measurements for this thread.
    pub network: Option<NetworkRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn forensic_measurements_distinguish_unavailable_from_measured_zero() {
        let unavailable = ForensicMemoryRecord::default();
        assert_eq!(unavailable.minor_page_faults, None);
        assert_eq!(unavailable.major_page_faults, None);
        assert_ne!(
            unavailable,
            ForensicMemoryRecord {
                minor_page_faults: Some(0),
                major_page_faults: None,
            }
        );
        assert_ne!(
            ForensicCpuRecord::default(),
            ForensicCpuRecord {
                runtime_ns: Some(0),
                utilization: None,
            }
        );
        let network = ForensicNetworkRecord::default();
        assert!(network.tcp.is_none());
        assert!(network.udp.is_none());
        assert!(network.rates.is_none());
    }

    #[test]
    fn process_state_can_change_without_changing_process_identity() {
        let running = ProcessRecord {
            process_identity: ProcessIdentity {
                pid: 42,
                start_time_ticks: 123,
                logical_label: None,
                security_label: None,
            },
            timestamp_unix_ns: 1,
            timestamp_monotonic_ns: 2,
            process_state: Some('R'),
            resource: ResourceRecord::Memory(MemoryRecord::default()),
        };
        let mut sleeping = running.clone();
        sleeping.process_state = Some('S');

        assert_eq!(running.process_identity, sleeping.process_identity);
        assert_ne!(running, sleeping);
    }

    #[test]
    fn default_cpu_counters_are_unavailable() {
        let process = CpuCounters::default();
        assert_eq!([process.user_ticks, process.system_ticks], [None; 2]);

        let system = SystemCpuCounters::default();
        assert_eq!(
            [
                system.user_ticks,
                system.nice_ticks,
                system.system_ticks,
                system.idle_ticks,
                system.iowait_ticks,
                system.irq_ticks,
                system.softirq_ticks,
                system.steal_ticks,
                system.guest_ticks,
                system.guest_nice_ticks,
            ],
            [None; 10]
        );
    }

    #[test]
    fn default_memory_measurements_are_unavailable() {
        let process = MemoryRecord::default();
        assert_eq!(
            [
                process.virtual_bytes,
                process.resident_bytes,
                process.proportional_bytes
            ],
            [None; 3]
        );

        let system = SystemMemoryRecord::default();
        assert_eq!([system.total_bytes, system.available_bytes], [None; 2]);
    }

    #[test]
    fn default_storage_counters_are_unavailable() {
        let process = StorageCounters::default();
        assert_eq!(
            [
                process.read_bytes,
                process.write_bytes,
                process.read_syscalls,
                process.write_syscalls
            ],
            [None; 4]
        );

        let system = SystemStorageCounters::default();
        assert_eq!(
            [
                system.read_operations,
                system.write_operations,
                system.read_sectors,
                system.write_sectors
            ],
            [None; 4]
        );
    }

    #[test]
    fn default_network_counters_are_unavailable() {
        let process = NetworkCounters::default();
        assert_eq!(
            [
                process.rx_bytes,
                process.tx_bytes,
                process.rx_packets,
                process.tx_packets
            ],
            [None; 4]
        );
        let tcp = ForensicTcpCounters::default();
        assert_eq!(
            [tcp.rx_bytes, tcp.tx_bytes, tcp.rx_messages, tcp.tx_messages],
            [None; 4]
        );
        let udp = ForensicUdpCounters::default();
        assert_eq!(
            [udp.rx_bytes, udp.tx_bytes, udp.rx_messages, udp.tx_messages],
            [None; 4]
        );
        let system = SystemNetworkCounters::default();
        assert_eq!(
            [
                system.rx_bytes,
                system.tx_bytes,
                system.rx_packets,
                system.tx_packets,
                system.rx_errors,
                system.tx_errors,
                system.rx_dropped,
                system.tx_dropped,
            ],
            [None; 8]
        );
    }

    #[test]
    fn measured_zero_differs_from_an_unavailable_measurement() {
        assert_ne!(
            MemoryRecord {
                resident_bytes: Some(0),
                ..Default::default()
            },
            MemoryRecord::default()
        );
        assert_ne!(
            CpuCounters {
                user_ticks: Some(0),
                ..Default::default()
            },
            CpuCounters::default()
        );
        assert_ne!(
            IoRates {
                interval_ns: 1_000_000_000,
                read_bytes_per_second: Some(0.0),
                write_bytes_per_second: None,
            },
            IoRates {
                interval_ns: 1_000_000_000,
                read_bytes_per_second: None,
                write_bytes_per_second: None,
            }
        );
    }

    #[test]
    fn block_device_keys_distinguish_major_minor_and_name() {
        let identity = SystemBlockDeviceIdentity {
            major: 8,
            minor: 0,
            name: "sda".into(),
        };
        let identities = [
            identity.clone(),
            SystemBlockDeviceIdentity {
                major: 9,
                ..identity.clone()
            },
            SystemBlockDeviceIdentity {
                minor: 1,
                ..identity.clone()
            },
            SystemBlockDeviceIdentity {
                name: "sdb".into(),
                ..identity.clone()
            },
        ];
        let baselines: HashMap<_, _> = identities.into_iter().zip(0..).collect();

        assert_eq!(baselines.len(), 4);
        assert_eq!(baselines.get(&identity), Some(&0));
    }

    #[test]
    fn network_interface_keys_distinguish_namespace_index_and_name() {
        let identity = SystemNetworkInterfaceIdentity {
            network_namespace_inode: 100,
            interface_index: 2,
            name: "eth0".into(),
        };
        let identities = [
            identity.clone(),
            SystemNetworkInterfaceIdentity {
                network_namespace_inode: 101,
                ..identity.clone()
            },
            SystemNetworkInterfaceIdentity {
                interface_index: 3,
                ..identity.clone()
            },
            SystemNetworkInterfaceIdentity {
                name: "eth1".into(),
                ..identity.clone()
            },
        ];
        let baselines: HashMap<_, _> = identities.into_iter().zip(0..).collect();

        assert_eq!(baselines.len(), 4);
        assert_eq!(baselines.get(&identity), Some(&0));
    }
}
