# ADR-0003: Trigger Process Capture with Aya and BPF Events

## Status

Proposed

## Context

Periodic procfs scans can find running processes, but a short-lived process may start and exit between scans. The per-process workers in ADR-0002 also need a way to learn about new processes promptly without making the discovery interval excessively short.

Linux exposes process lifecycle events through scheduler tracepoints. A BPF program can observe these events in the kernel and notify the user-space observer.

## Decision

Use Aya to load and attach BPF programs to process fork, successful exec, and exit tracepoints (`sched_process_fork`, `sched_process_exec`, and `sched_process_exit`). 
Send small event records to the Rust observer through a BPF ring buffer. An event will contain its type and the process identifiers needed to route it; the BPF program will not collect a full process snapshot or send data to the backend.

Run a Tokio task to receive events from Aya's `RingBuf` using Tokio's `AsyncFd`. This reader will hand events to the process coordinator without doing procfs reads itself. On fork or exec, the coordinator will apply process-selection rules and verify the PID and process start time. It will launch a new Tokio sampling worker for a newly tracked process, or request a prompt snapshot from its existing worker. Duplicate events for the same process will be coalesced; they will not create a worker per event. Event-triggered reads will share the concurrency limit used by periodic workers in ADR-0002.

On exit, the observer will stop the matching worker and record the lifecycle event. It will not try to read a final procfs snapshot after exit, because the process may already be gone.

At startup, attach the BPF programs before scanning procfs for already-running processes, then deduplicate scan results against received events. Continue periodic reconciliation scans so a full ring buffer, a missed event, or temporary BPF unavailability cannot leave the tracked process set permanently out of date. Aya's ring-buffer map requires Linux 5.8 or later; if it or a required tracepoint cannot be used on a target device, continue with procfs discovery and periodic sampling, and report that event capture is unavailable.

## Rationale

Kernel lifecycle events reduce discovery latency for new processes, including short-lived ones. Aya lets the Rust observer load the BPF programs and consume their events, while Tokio runs the reader and per-process workers. Keeping BPF limited to event notification leaves process selection, snapshot collection, and backend communication in user space, where the existing observer logic can handle them.

## Consequences

### Positive

- New processes can be considered for capture without waiting for the next discovery scan.
- Exit events can stop workers promptly.
- Spreading out and limiting simultaneous snapshots avoids a thundering herd when many process events arrive at once.
- Procfs remains the source of process snapshots and the fallback for discovery.

### Negative

- BPF support, permissions, and available tracepoints vary across target kernels and deployments.
- Ring-buffer events can be lost under load; reconciliation remains necessary.
- If many events arrive at once, the observer must combine repeated requests for the same process and limit how many snapshots run at the same time.

## References

- [Linux scheduler tracepoint definitions](https://github.com/torvalds/linux/blob/master/include/trace/events/sched.h)
- [Linux kernel documentation: BPF ring buffer](https://docs.kernel.org/bpf/ringbuf.html)
- [Aya tracepoint programs](https://docs.rs/aya/latest/aya/programs/trace_point/struct.TracePoint.html)
- [Aya ring buffer](https://docs.rs/aya/latest/aya/maps/ring_buf/struct.RingBuf.html)
- [ADR-0002: Schedule Per-Process Sampling Workers](0002-schedule-per-process-sampling-workers.md)
