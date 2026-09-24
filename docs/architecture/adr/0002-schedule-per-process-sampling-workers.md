# ADR-0002: Schedule Per-Process Sampling Workers

## Status

Proposed

## Context

The observer may track many processes at once. Sampling every process on the same fixed timer could cause bursts of procfs reads, CPU work, and records sent to the backend. Processes also change at different rates, so a single sampling frequency would either miss short-lived changes or spend unnecessary work on stable processes.

Processes can exit between discovery and sampling, and Linux can reuse a PID for a different process.

## Decision

Use one independent Tokio task for each tracked process. A coordinator discovers eligible processes, starts workers, and stops them when a process exits or is no longer selected for tracking. Identify a worker by its PID and the process start time reported in `/proc/<pid>/stat`, rather than by PID alone.

Each worker will:

1. Apply a bounded, randomized delay to its periodic schedule so newly discovered processes do not all sample at once. An event-triggered first snapshot may run promptly, subject to the shared concurrency limit in ADR-0003.
2. Collect a process snapshot when due, subject to a shared limit on concurrent procfs reads.
3. Compare consecutive valid snapshots to assess how much the process is changing.
4. Shorten its sampling interval when change is significant, and lengthen it after repeated stable samples. Keep the interval within configured minimum and maximum bounds so every tracked process is checked periodically.
5. Add bounded jitter to later deadlines to avoid workers becoming synchronized again. Schedule the next read from the current time so delayed work does not trigger a burst of catch-up samples.

Treat the first snapshot as a baseline. If the process exits, its start time changes, or a counter resets, do not calculate a change from the previous snapshot. The specific change thresholds and interval bounds will be chosen from measurements on target devices.

## Rationale

Independent workers keep scheduling state local to each process. Jitter spreads procfs reads over time, while the shared concurrency limit caps the work that can happen at once. Adaptive intervals reserve more samples for changing processes without allowing stable processes to go unobserved indefinitely.

## Consequences

### Positive

- Sampling load is spread over time rather than concentrated at one timer boundary.
- Frequently changing processes can be observed more closely than stable ones.
- PID reuse does not silently attach an existing worker's history to a new process.

### Negative

- Each tracked process adds a task, timer, and scheduling state; resource use must be measured at the expected process count.
- Adaptive sampling makes observation intervals uneven and may miss changes that occur between samples.
- Worker lifecycle and shutdown need to be coordinated with process discovery.

## References

- [Tokio interval and missed tick behavior](https://docs.rs/tokio/latest/tokio/time/fn.interval.html)
- [Linux kernel documentation: procfs process statistics](https://docs.kernel.org/filesystems/proc.html)
- [ADR-0003: Trigger Process Capture with Aya and BPF Events](0003-trigger-process-capture-with-bpf-events.md)
