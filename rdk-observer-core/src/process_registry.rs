//! Registry of process targets and collection state.
//!
//! This module will maintain selected targets using PID and the start time
//! reported in `/proc/<pid>/stat`, following ADR-0002. PID reuse will not attach
//! an old target's collection state to a new process.
//!
//! `process_worker` will use the registry to add, update, and remove targets,
//! deduplicate discovery results and events, and track collection task handles.
//! `collector_worker` will use registered identities to collect one process or
//! sample the selected list. Observation records will belong to `output_*`.

#[derive(Default)]
pub struct ProcessRegistry {
    // todo: this is a stub implementation
}
