//! Core observation types and behavior for RDK Observer.
//!
//! These modules will implement process discovery, resource collection, and
//! Protobuf output using the plans in ADRs 0001-0004.
//!
//! `process_*` will establish the targets to monitor. `process_discovery` will
//! find candidates through an initial procfs inventory, reconciliation every
//! 30 seconds, and netlink process connector lifecycle events;
//! `process_worker` will apply selection rules, maintain `process_registry`, and
//! coordinate collection for registered PID/start-time identities.
//!
//! Startup will use `normal-observer-mode` without loading or attaching eBPF.
//! Anomaly detection will activate targeted process or thread eBPF analysis in
//! `forensic-observer-mode`, while normal discovery and collection continue.
//! `runtime` will coordinate forensic sessions and release eBPF resources when
//! sessions end, following ADR-0003.
//!
//! `collector_*` will gather measurements for individual registered targets, a
//! full pass over the selected process list, or system totals, according to
//! configuration and source support. Tasks will send timestamped observations
//! directly to `output_worker`. Discovery scans will identify targets; collection
//! passes will read their measurements. Neither will imply an atomic snapshot
//! of the whole system.
//!
//! `output_*` will prepare collected observations, encode them using
//! `output_model` and `output_encoding`, and buffer them through `output_storage`.
//! Process lifecycle events will drive discovery and collection coordination.
//! Consumption or delivery of buffered records will be defined separately.
//! `runtime` will start the workers selected by the input configuration, connect
//! them, and manage their lifecycle.

pub mod collector_cpu;
pub mod collector_io;
pub mod collector_memory;
pub mod collector_network;
pub mod collector_pressure;
pub mod collector_worker;
pub mod config;
pub mod output_encoding;
pub mod output_model;
pub mod output_storage;
pub mod output_worker;
pub mod process_discovery;
pub mod process_registry;
pub mod process_worker;
pub mod runtime;
pub mod runtime_components;
