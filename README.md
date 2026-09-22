# rdk-observer-rs

`rdk-observer-rs` is a system process observer for RDK devices. Its goal is to inspect running processes, create records from the observations, and send those records to a backend for analysis and monitoring.

Records will be encoded in CBOR (Concise Binary Object Representation), a compact binary format intended to keep payloads small and reduce the cost of sending process data.

## Planned tracking options

The observer is intended to support multiple ways to choose what to track:

- All user-space processes.
- Kernel processes and threads, where the system exposes them.
- Processes associated with RDK-B.
- Individually selected processes.

These options will allow deployments to choose between a broad system view and focused monitoring of specific processes.

This repository is currently a project scaffold; the observer and its tracking options are planned functionality.
