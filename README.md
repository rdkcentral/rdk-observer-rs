# rdk-observer-rs

`rdk-observer-rs` is a system process observer for RDK devices. Its goal is to inspect running processes, create records from the observations, and send those records to a backend for analysis and monitoring.

## Planned tracking options

The observer is intended to support multiple ways to choose what to track:

- All user-space processes.
- Kernel processes and threads, where the system exposes them.
- Processes associated with RDK-B.
- Individually selected processes.

These options will allow deployments to choose between a broad system view and focused monitoring of specific processes.

This repository is currently a project scaffold; the observer and its tracking options are planned functionality.

## Architecture decisions

- [ADR-0001: Collect System Resource Snapshots from procfs](docs/architecture/adr/0001-collect-system-resource-snapshots-from-procfs.md)
- [ADR-0002: Schedule Per-Process Sampling Workers](docs/architecture/adr/0002-schedule-per-process-sampling-workers.md)
- [ADR-0003: Trigger Process Capture with Aya and BPF Events](docs/architecture/adr/0003-trigger-process-capture-with-bpf-events.md)
- [ADR-0004: Use CBOR for Observation Records](docs/architecture/adr/0004-use-cbor-for-observation-records.md)
