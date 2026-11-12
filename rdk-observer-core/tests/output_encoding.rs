use prost::Message;
use rdk_observer_core::output_encoding::{
    DecodeError, decode_process_record, decode_system_record, encode_process_record,
    encode_system_record, proto,
};
use rdk_observer_core::output_model::*;

fn observation(resource: ResourceRecord) -> ProcessRecord {
    ProcessRecord {
        process_identity: ProcessIdentity {
            pid: u32::MAX,
            start_time_ticks: u64::MAX,
            logical_label: Some(String::new()),
            security_label: Some(ProcessSecurityLabel {
                module: "selinux".into(),
                value: "system_u:system_r:observer_t:s0 🦀".into(),
            }),
        },
        timestamp_unix_ns: u64::MAX - 1,
        timestamp_monotonic_ns: u64::MAX - 2,
        process_state: None,
        resource,
    }
}

fn cpu() -> CpuRecord {
    CpuRecord {
        clock_ticks_per_second: 100,
        counters: Some(CpuCounters {
            user_ticks: Some(0),
            system_ticks: Some(u64::MAX),
        }),
        utilization: Some(CpuUtilization {
            interval_ns: u64::MAX,
            percent: 125.5,
        }),
        forensic: None,
    }
}

fn storage() -> StorageRecord {
    StorageRecord {
        counters: StorageCounters {
            read_bytes: Some(0),
            write_bytes: Some(u64::MAX),
            read_syscalls: None,
            write_syscalls: Some(0),
        },
        rates: Some(IoRates {
            interval_ns: u64::MAX,
            read_bytes_per_second: Some(0.0),
            write_bytes_per_second: None,
        }),
    }
}

#[test]
fn process_resources_round_trip_with_labels_and_integer_limits() {
    let records = [
        ResourceRecord::Memory(MemoryRecord {
            virtual_bytes: None,
            resident_bytes: Some(0),
            proportional_bytes: Some(u64::MAX),
            forensic: None,
        }),
        ResourceRecord::Cpu(cpu()),
        ResourceRecord::Storage(storage()),
        ResourceRecord::Network(NetworkRecord {
            counters: Some(NetworkCounters {
                rx_bytes: Some(1),
                tx_bytes: Some(2),
                rx_packets: Some(3),
                tx_packets: Some(4),
            }),
            rates: Some(NetworkRates {
                interval_ns: u64::MAX,
                rx_bytes_per_second: Some(0.0),
                tx_bytes_per_second: None,
            }),
            forensic: Some(ForensicNetworkRecord {
                tcp: Some(ForensicTcpCounters {
                    rx_bytes: Some(u64::MAX),
                    tx_bytes: Some(0),
                    rx_messages: None,
                    tx_messages: Some(u64::MAX),
                }),
                udp: Some(ForensicUdpCounters {
                    rx_bytes: Some(0),
                    tx_bytes: Some(u64::MAX),
                    rx_messages: Some(u64::MAX),
                    tx_messages: None,
                }),
                rates: None,
            }),
        }),
        ResourceRecord::Threads(vec![
            ThreadRecord {
                identity: ThreadIdentity {
                    tid: u32::MAX,
                    start_time_ticks: u64::MAX,
                },
                name: Some(String::new()),
                state: Some('🦀'),
                priority: Some(i64::MIN),
                nice: Some(i32::MIN),
                last_cpu: Some(0),
                voluntary_context_switches: Some(u64::MAX),
                nonvoluntary_context_switches: Some(0),
                cpu: Some(cpu()),
                storage: Some(storage()),
                memory: None,
                network: None,
            },
            ThreadRecord {
                identity: ThreadIdentity {
                    tid: 0,
                    start_time_ticks: 0,
                },
                name: None,
                state: None,
                priority: Some(i64::MAX),
                nice: Some(i32::MAX),
                last_cpu: None,
                voluntary_context_switches: None,
                nonvoluntary_context_switches: None,
                cpu: None,
                storage: None,
                memory: None,
                network: None,
            },
        ]),
        ResourceRecord::Threads(vec![]),
    ];
    for resource in records {
        let mut record = observation(resource);
        record.process_state = Some('S');
        assert_eq!(
            decode_process_record(&encode_process_record(&record)).unwrap(),
            record
        );
    }
}

#[test]
fn shared_forensic_records_round_trip_for_process_and_thread_scopes() {
    let capture_cpu = ForensicCpuRecord {
        runtime_ns: Some(u64::MAX),
        utilization: Some(ForensicCpuUtilization {
            interval_ns: u64::MAX,
            percent: 37.5,
        }),
    };
    let capture_memory = ForensicMemoryRecord {
        minor_page_faults: Some(0),
        major_page_faults: Some(u64::MAX),
    };
    let capture_network = ForensicNetworkRecord {
        tcp: Some(ForensicTcpCounters {
            rx_bytes: Some(u64::MAX),
            tx_bytes: Some(0),
            rx_messages: None,
            tx_messages: Some(1),
        }),
        udp: Some(ForensicUdpCounters {
            rx_bytes: Some(0),
            tx_bytes: None,
            rx_messages: Some(2),
            tx_messages: Some(u64::MAX),
        }),
        rates: Some(ForensicNetworkRates {
            interval_ns: u64::MAX,
            rx_bytes_per_second: Some(0.0),
            tx_bytes_per_second: None,
        }),
    };
    let mut combined_cpu = cpu();
    combined_cpu.forensic = Some(capture_cpu);
    let capture_only_cpu = CpuRecord {
        clock_ticks_per_second: 0,
        counters: None,
        utilization: None,
        forensic: Some(capture_cpu),
    };
    let capture_only_network = NetworkRecord {
        counters: None,
        rates: None,
        forensic: Some(capture_network.clone()),
    };
    let records = [
        ResourceRecord::Cpu(combined_cpu),
        ResourceRecord::Cpu(capture_only_cpu.clone()),
        ResourceRecord::Memory(MemoryRecord {
            virtual_bytes: Some(4096),
            resident_bytes: Some(2048),
            proportional_bytes: Some(1024),
            forensic: Some(capture_memory),
        }),
        ResourceRecord::Memory(MemoryRecord {
            forensic: Some(capture_memory),
            ..Default::default()
        }),
        ResourceRecord::Network(NetworkRecord {
            counters: Some(NetworkCounters {
                rx_bytes: Some(17),
                ..Default::default()
            }),
            rates: Some(NetworkRates {
                interval_ns: 1000,
                rx_bytes_per_second: Some(50.0),
                tx_bytes_per_second: Some(25.0),
            }),
            forensic: Some(capture_network),
        }),
        ResourceRecord::Network(capture_only_network.clone()),
        ResourceRecord::Threads(vec![ThreadRecord {
            identity: ThreadIdentity {
                tid: 43,
                start_time_ticks: 100,
            },
            name: Some("worker".into()),
            state: Some('S'),
            priority: None,
            nice: None,
            last_cpu: Some(0),
            voluntary_context_switches: Some(0),
            nonvoluntary_context_switches: None,
            cpu: Some(capture_only_cpu),
            memory: Some(capture_memory),
            storage: Some(storage()),
            network: Some(capture_only_network),
        }]),
    ];

    for resource in records {
        let record = observation(resource);
        let bytes = encode_process_record(&record);
        assert_eq!(decode_process_record(&bytes).unwrap(), record);
    }
}

#[test]
fn forensic_only_cpu_wire_fixture_preserves_present_zero_runtime() {
    // Empty identity; CPU at tag 11; forensic at tag 4; present-zero runtime at tag 1.
    let bytes = [0x0a, 0x00, 0x5a, 0x04, 0x22, 0x02, 0x08, 0x00];
    let record = decode_process_record(&bytes).unwrap();
    assert_eq!(
        record.resource,
        ResourceRecord::Cpu(CpuRecord {
            clock_ticks_per_second: 0,
            counters: None,
            utilization: None,
            forensic: Some(ForensicCpuRecord {
                runtime_ns: Some(0),
                utilization: None
            }),
        })
    );
    assert_eq!(encode_process_record(&record), bytes);
}

#[test]
fn forensic_only_memory_wire_fixture_preserves_fault_counts_and_absent_sizes() {
    // Empty identity; memory at tag 10; forensic at tag 6; minor/major faults at tags 1/2.
    let bytes = [0x0a, 0x00, 0x52, 0x06, 0x32, 0x04, 0x08, 0x00, 0x10, 0x07];
    let record = decode_process_record(&bytes).unwrap();
    assert_eq!(
        record.resource,
        ResourceRecord::Memory(MemoryRecord {
            virtual_bytes: None,
            resident_bytes: None,
            proportional_bytes: None,
            forensic: Some(ForensicMemoryRecord {
                minor_page_faults: Some(0),
                major_page_faults: Some(7)
            }),
        })
    );
    assert_eq!(encode_process_record(&record), bytes);
}

#[test]
fn system_resources_round_trip_without_losing_devices_or_pressure_values() {
    let pressure = SystemPressureResourceRecord {
        some: SystemPressureStallRecord {
            avg10_percent: 1.25,
            avg60_percent: 2.5,
            avg300_percent: 3.75,
            total_microseconds: u64::MAX,
        },
        full: SystemPressureStallRecord {
            avg10_percent: 0.0,
            avg60_percent: 0.125,
            avg300_percent: 100.0,
            total_microseconds: 0,
        },
    };
    let records = [
        SystemResourceRecord::Cpu(SystemCpuRecord {
            clock_ticks_per_second: u64::MAX,
            counters: SystemCpuCounters {
                user_ticks: Some(1),
                nice_ticks: Some(2),
                system_ticks: Some(3),
                idle_ticks: Some(4),
                iowait_ticks: Some(5),
                irq_ticks: Some(6),
                softirq_ticks: Some(7),
                steal_ticks: Some(8),
                guest_ticks: Some(9),
                guest_nice_ticks: Some(10),
            },
            utilization: Some(SystemCpuUtilization {
                interval_ns: 1,
                percent: 42.5,
            }),
        }),
        SystemResourceRecord::Memory(SystemMemoryRecord {
            total_bytes: Some(u64::MAX),
            available_bytes: Some(0),
        }),
        SystemResourceRecord::Storage(vec![
            SystemStorageRecord {
                device_identity: SystemBlockDeviceIdentity {
                    major: u32::MAX,
                    minor: u32::MAX,
                    name: "mmcblk0".into(),
                },
                counters: SystemStorageCounters {
                    read_operations: Some(1),
                    write_operations: Some(2),
                    read_sectors: Some(u64::MAX),
                    write_sectors: Some(0),
                },
                rates: storage().rates,
            },
            SystemStorageRecord {
                device_identity: SystemBlockDeviceIdentity {
                    major: 8,
                    minor: 0,
                    name: "sda".into(),
                },
                counters: SystemStorageCounters::default(),
                rates: None,
            },
        ]),
        SystemResourceRecord::Storage(vec![]),
        SystemResourceRecord::Network(vec![SystemNetworkRecord {
            interface_identity: SystemNetworkInterfaceIdentity {
                network_namespace_inode: u64::MAX,
                interface_index: u32::MAX,
                name: "eth0".into(),
            },
            counters: SystemNetworkCounters {
                rx_bytes: Some(u64::MAX),
                tx_bytes: Some(0),
                rx_packets: Some(1),
                tx_packets: Some(2),
                rx_errors: None,
                tx_errors: Some(3),
                rx_dropped: Some(4),
                tx_dropped: Some(5),
            },
            rates: Some(NetworkRates {
                interval_ns: 1_000_000_000,
                rx_bytes_per_second: None,
                tx_bytes_per_second: Some(25.5),
            }),
        }]),
        SystemResourceRecord::Network(vec![]),
        SystemResourceRecord::Pressure(SystemPressureRecord {
            cpu: Some(pressure.clone()),
            memory: None,
            io: Some(pressure),
        }),
        SystemResourceRecord::Pressure(SystemPressureRecord {
            cpu: None,
            memory: None,
            io: None,
        }),
    ];
    for resource in records {
        let record = SystemRecord {
            timestamp_unix_ns: u64::MAX,
            timestamp_monotonic_ns: u64::MAX - 1,
            resource,
        };
        assert_eq!(
            decode_system_record(&encode_system_record(&record)).unwrap(),
            record
        );
    }
}

#[test]
fn unavailable_measurements_and_labels_remain_absent() {
    for resource in [
        ResourceRecord::Cpu(CpuRecord {
            clock_ticks_per_second: 0,
            counters: Some(CpuCounters::default()),
            utilization: None,
            forensic: None,
        }),
        ResourceRecord::Memory(MemoryRecord::default()),
        ResourceRecord::Network(NetworkRecord {
            counters: Some(NetworkCounters::default()),
            rates: None,
            forensic: None,
        }),
        ResourceRecord::Network(NetworkRecord {
            counters: Some(NetworkCounters::default()),
            rates: None,
            forensic: Some(ForensicNetworkRecord {
                tcp: Some(ForensicTcpCounters::default()),
                udp: Some(ForensicUdpCounters::default()),
                rates: None,
            }),
        }),
        ResourceRecord::Storage(StorageRecord {
            counters: StorageCounters::default(),
            rates: None,
        }),
    ] {
        let mut record = observation(resource);
        record.process_identity.logical_label = None;
        record.process_identity.security_label = None;
        assert_eq!(
            decode_process_record(&encode_process_record(&record)).unwrap(),
            record
        );
    }
    for resource in [
        SystemResourceRecord::Cpu(SystemCpuRecord {
            clock_ticks_per_second: 0,
            counters: SystemCpuCounters::default(),
            utilization: None,
        }),
        SystemResourceRecord::Memory(SystemMemoryRecord::default()),
    ] {
        let record = SystemRecord {
            timestamp_unix_ns: 0,
            timestamp_monotonic_ns: 0,
            resource,
        };
        assert_eq!(
            decode_system_record(&encode_system_record(&record)).unwrap(),
            record
        );
    }
}

#[test]
fn process_memory_wire_fixture_preserves_field_numbers_and_present_zero() {
    // Independently specified message: PID 42, start tick 300, timestamps 7/8,
    // absent virtual size, resident zero, proportional size 9.
    let bytes = [
        0x0a, 0x05, 0x08, 0x2a, 0x10, 0xac, 0x02, 0x10, 0x07, 0x18, 0x08, 0x52, 0x04, 0x10, 0x00,
        0x18, 0x09,
    ];
    let expected = ProcessRecord {
        process_identity: ProcessIdentity {
            pid: 42,
            start_time_ticks: 300,
            logical_label: None,
            security_label: None,
        },
        timestamp_unix_ns: 7,
        timestamp_monotonic_ns: 8,
        process_state: None,
        resource: ResourceRecord::Memory(MemoryRecord {
            virtual_bytes: None,
            resident_bytes: Some(0),
            proportional_bytes: Some(9),
            forensic: None,
        }),
    };
    assert_eq!(decode_process_record(&bytes).unwrap(), expected);
    assert_eq!(encode_process_record(&expected), bytes);
}

#[test]
fn process_state_wire_fixture_uses_optional_field_four() {
    // Empty process identity, process state "S" at tag 4, empty memory at tag 10.
    let bytes = [0x0a, 0x00, 0x22, 0x01, 0x53, 0x52, 0x00];
    let expected = ProcessRecord {
        process_identity: ProcessIdentity {
            pid: 0,
            start_time_ticks: 0,
            logical_label: None,
            security_label: None,
        },
        timestamp_unix_ns: 0,
        timestamp_monotonic_ns: 0,
        process_state: Some('S'),
        resource: ResourceRecord::Memory(MemoryRecord::default()),
    };

    assert_eq!(decode_process_record(&bytes).unwrap(), expected);
    assert_eq!(encode_process_record(&expected), bytes);
}

#[test]
fn system_memory_wire_fixture_preserves_field_numbers_and_present_zero() {
    let bytes = [0x08, 0x01, 0x10, 0x02, 0x5a, 0x02, 0x08, 0x00];
    let expected = SystemRecord {
        timestamp_unix_ns: 1,
        timestamp_monotonic_ns: 2,
        resource: SystemResourceRecord::Memory(SystemMemoryRecord {
            total_bytes: Some(0),
            available_bytes: None,
        }),
    };
    assert_eq!(decode_system_record(&bytes).unwrap(), expected);
    assert_eq!(encode_system_record(&expected), bytes);
}

#[test]
fn tcp_wire_fixture_preserves_counters_separately_from_generic_network_totals() {
    // Process identity present; resource tag 14; empty generic counters at tag 1;
    // Forensic at tag 5 contains TCP at tag 1, with RX/TX bytes 7/9 and operations 1/2.
    let bytes = [
        0x0a, 0x00, 0x72, 0x0e, 0x0a, 0x00, 0x2a, 0x0a, 0x0a, 0x08, 0x08, 0x07, 0x10, 0x09, 0x18,
        0x01, 0x20, 0x02,
    ];
    let mut expected = observation(ResourceRecord::Network(NetworkRecord {
        counters: Some(NetworkCounters::default()),
        rates: None,
        forensic: Some(ForensicNetworkRecord {
            tcp: Some(ForensicTcpCounters {
                rx_bytes: Some(7),
                tx_bytes: Some(9),
                rx_messages: Some(1),
                tx_messages: Some(2),
            }),
            udp: None,
            rates: None,
        }),
    }));
    expected.process_identity = ProcessIdentity {
        pid: 0,
        start_time_ticks: 0,
        logical_label: None,
        security_label: None,
    };
    expected.timestamp_unix_ns = 0;
    expected.timestamp_monotonic_ns = 0;
    assert_eq!(decode_process_record(&bytes).unwrap(), expected);
    assert_eq!(encode_process_record(&expected), bytes);
}

#[test]
fn udp_wire_fixture_preserves_zero_byte_datagram_counts() {
    // Network tag 14, empty generic counters at tag 1, forensic at tag 5 with UDP at tag 2.
    // RX/TX payload bytes are zero, while RX/TX operation counts are 1/2.
    let bytes = [
        0x0a, 0x00, 0x72, 0x0e, 0x0a, 0x00, 0x2a, 0x0a, 0x12, 0x08, 0x08, 0x00, 0x10, 0x00, 0x18,
        0x01, 0x20, 0x02,
    ];
    let record = decode_process_record(&bytes).unwrap();
    assert_eq!(
        record.resource,
        ResourceRecord::Network(NetworkRecord {
            counters: Some(NetworkCounters::default()),
            rates: None,
            forensic: Some(ForensicNetworkRecord {
                tcp: None,
                udp: Some(ForensicUdpCounters {
                    rx_bytes: Some(0),
                    tx_bytes: Some(0),
                    rx_messages: Some(1),
                    tx_messages: Some(2),
                }),
                rates: None
            }),
        })
    );
    assert_eq!(encode_process_record(&record), bytes);
}

#[test]
fn network_wire_fixture_without_protocol_counters_preserves_absence_and_present_zero() {
    // Existing network fields retain their numbers, with no TCP/UDP payload.
    let bytes = [0x0a, 0x00, 0x72, 0x06, 0x0a, 0x04, 0x08, 0x00, 0x10, 0x09];
    let record = decode_process_record(&bytes).unwrap();
    assert_eq!(
        record.resource,
        ResourceRecord::Network(NetworkRecord {
            counters: Some(NetworkCounters {
                rx_bytes: Some(0),
                tx_bytes: Some(9),
                ..Default::default()
            }),
            rates: None,
            forensic: None,
        })
    );
    assert_eq!(encode_process_record(&record), bytes);
}

#[test]
fn optional_zero_rates_are_distinct_from_unavailable_rates_on_the_wire() {
    let record = observation(ResourceRecord::Storage(storage()));
    let wire = proto::ProcessRecord::decode(encode_process_record(&record).as_slice()).unwrap();
    let Some(proto::process_record::Resource::Storage(storage)) = wire.resource else {
        panic!("expected storage resource");
    };
    let rates = storage.rates.unwrap();
    assert_eq!(rates.read_bytes_per_second, Some(0.0));
    assert_eq!(rates.write_bytes_per_second, None);
}

#[test]
fn missing_top_level_fields_are_rejected() {
    assert!(matches!(
        decode_process_record(&[]),
        Err(DecodeError::MissingField("ProcessRecord.process_identity"))
    ));
    let wire = proto::ProcessRecord {
        process_identity: Some(Default::default()),
        ..Default::default()
    };
    assert!(matches!(
        decode_process_record(&wire.encode_to_vec()),
        Err(DecodeError::MissingField("ProcessRecord.resource"))
    ));
    assert!(matches!(
        decode_system_record(&[]),
        Err(DecodeError::MissingField("SystemRecord.resource"))
    ));
}

#[test]
fn missing_nested_measurements_and_identities_are_rejected() {
    use proto::process_record::Resource;
    for (resource, field) in [
        (
            Resource::Cpu(Default::default()),
            "CpuRecord.counters or CpuRecord.forensic",
        ),
        (
            Resource::Network(Default::default()),
            "NetworkRecord.counters or NetworkRecord.forensic",
        ),
        (
            Resource::Storage(Default::default()),
            "StorageRecord.counters",
        ),
        (
            Resource::Threads(proto::ThreadRecords {
                entries: vec![Default::default()],
            }),
            "ThreadRecord.identity",
        ),
        (
            Resource::Threads(proto::ThreadRecords {
                entries: vec![proto::ThreadRecord {
                    identity: Some(Default::default()),
                    cpu: Some(Default::default()),
                    ..Default::default()
                }],
            }),
            "CpuRecord.counters or CpuRecord.forensic",
        ),
        (
            Resource::Threads(proto::ThreadRecords {
                entries: vec![proto::ThreadRecord {
                    identity: Some(Default::default()),
                    storage: Some(Default::default()),
                    ..Default::default()
                }],
            }),
            "StorageRecord.counters",
        ),
        (
            Resource::Threads(proto::ThreadRecords {
                entries: vec![proto::ThreadRecord {
                    identity: Some(Default::default()),
                    network: Some(Default::default()),
                    ..Default::default()
                }],
            }),
            "NetworkRecord.counters or NetworkRecord.forensic",
        ),
    ] {
        let wire = proto::ProcessRecord {
            process_identity: Some(Default::default()),
            resource: Some(resource),
            ..Default::default()
        };
        assert!(matches!(decode_process_record(&wire.encode_to_vec()),
            Err(DecodeError::MissingField(actual)) if actual == field));
    }
    use proto::system_record::Resource as SystemResource;
    for (resource, field) in [
        (
            SystemResource::Cpu(Default::default()),
            "SystemCpuRecord.counters",
        ),
        (
            SystemResource::Network(proto::SystemNetworkRecords {
                entries: vec![Default::default()],
            }),
            "SystemNetworkRecord.interface_identity",
        ),
        (
            SystemResource::Network(proto::SystemNetworkRecords {
                entries: vec![proto::SystemNetworkRecord {
                    interface_identity: Some(Default::default()),
                    ..Default::default()
                }],
            }),
            "SystemNetworkRecord.counters",
        ),
        (
            SystemResource::Storage(proto::SystemStorageRecords {
                entries: vec![Default::default()],
            }),
            "SystemStorageRecord.device_identity",
        ),
        (
            SystemResource::Storage(proto::SystemStorageRecords {
                entries: vec![proto::SystemStorageRecord {
                    device_identity: Some(Default::default()),
                    ..Default::default()
                }],
            }),
            "SystemStorageRecord.counters",
        ),
        (
            SystemResource::Pressure(proto::SystemPressureRecord {
                cpu: Some(Default::default()),
                ..Default::default()
            }),
            "SystemPressureResourceRecord.some",
        ),
        (
            SystemResource::Pressure(proto::SystemPressureRecord {
                memory: Some(proto::SystemPressureResourceRecord {
                    some: Some(Default::default()),
                    full: None,
                }),
                ..Default::default()
            }),
            "SystemPressureResourceRecord.full",
        ),
        (
            SystemResource::Pressure(proto::SystemPressureRecord {
                io: Some(Default::default()),
                ..Default::default()
            }),
            "SystemPressureResourceRecord.some",
        ),
    ] {
        let wire = proto::SystemRecord {
            resource: Some(resource),
            ..Default::default()
        };
        assert!(matches!(decode_system_record(&wire.encode_to_vec()),
            Err(DecodeError::MissingField(actual)) if actual == field));
    }
}

#[test]
fn thread_state_must_be_one_unicode_scalar() {
    for state in ["", "RS", "e\u{301}"] {
        let wire = proto::ProcessRecord {
            process_identity: Some(Default::default()),
            resource: Some(proto::process_record::Resource::Threads(
                proto::ThreadRecords {
                    entries: vec![proto::ThreadRecord {
                        identity: Some(Default::default()),
                        state: Some(state.into()),
                        ..Default::default()
                    }],
                },
            )),
            ..Default::default()
        };
        assert!(matches!(decode_process_record(&wire.encode_to_vec()),
            Err(DecodeError::InvalidThreadState(actual)) if actual == state));
    }
}

#[test]
fn malformed_and_truncated_protobuf_are_rejected() {
    for bytes in [&[0xff][..], &[0x0a, 0x02, 0x08][..]] {
        assert!(matches!(
            decode_process_record(bytes),
            Err(DecodeError::Protobuf(_))
        ));
        assert!(matches!(
            decode_system_record(bytes),
            Err(DecodeError::Protobuf(_))
        ));
    }
}

#[test]
fn unknown_fields_do_not_prevent_decoding_known_resources() {
    let record = observation(ResourceRecord::Memory(MemoryRecord::default()));
    let mut bytes = encode_process_record(&record);
    // Unknown field 100, varint value 42.
    bytes.extend_from_slice(&[0xa0, 0x06, 0x2a]);
    assert_eq!(decode_process_record(&bytes).unwrap(), record);
    let record = SystemRecord {
        timestamp_unix_ns: 1,
        timestamp_monotonic_ns: 2,
        resource: SystemResourceRecord::Memory(SystemMemoryRecord::default()),
    };
    let mut bytes = encode_system_record(&record);
    bytes.extend_from_slice(&[0xa0, 0x06, 0x2a]);
    assert_eq!(decode_system_record(&bytes).unwrap(), record);
}
