# ADR-0003: Discover Processes with procfs and Netlink; Reserve eBPF for Forensics

## Status

Proposed

## Context

Periodic procfs scans can find running processes, but a short-lived process may start and exit between scans. The per-process workers in ADR-0002 also need prompt lifecycle notifications without making full discovery scans excessively frequent.

Normal observation needs an inventory and routine resource measurements. Detailed investigation of anomalous processes or threads needs additional instrumentation. These responsibilities should have separate activation policies so ordinary process discovery does not depend on eBPF.

## Decision

### Normal observer mode

Start in `normal-observer-mode`, using procfs inventory snapshots and the Linux netlink process connector as complementary discovery sources. Netlink is the normal lifecycle event source. Do not load or attach eBPF programs in this mode.

1. Open a `NETLINK_CONNECTOR` socket, subscribe to the process connector group, and request `PROC_CN_MCAST_LISTEN` for `CN_IDX_PROC` / `CN_VAL_PROC`. Establish the subscription before the startup scan when available, and consume events while the scan runs.
2. Take an initial inventory snapshot by enumerating `/proc` and reading the identity information needed to apply process-selection rules. Identify registered processes by PID (thread-group ID) and the start time from `/proc/<pid>/stat`.
3. Reconcile the inventory with a full procfs discovery scan every 30 seconds. Add missed targets, refresh metadata, and remove entries confirmed to have exited or changed identity. A partial scan or permission failure is not proof of exit. Reconcile scan results with concurrent events so an older scan cannot restore an exited identity or remove a newly discovered one. Serialize scans; delayed scans must not cause overlapping or catch-up bursts.
4. Between scans, consume `PROC_EVENT_FORK`, `PROC_EVENT_EXEC`, and `PROC_EVENT_EXIT` notifications. Forward decoded lifecycle events to `process_worker`; the event reader will not collect resource snapshots itself.

On fork or successful exec, the coordinator will apply selection rules, verify process identity through procfs, and start a sampling worker for a newly tracked process or request a prompt sample from its existing worker. Exec will also refresh selection metadata. Repeated events and scan results for the same identity will be coalesced. Prompt resource reads will share the concurrency limit used by the periodic workers in ADR-0002.

The connector reports task IDs and thread-group IDs. Normal discovery will register process targets by thread-group identity, rather than create a process worker for every thread. An exit notification is a hint to resolve the matching task's lifecycle: a single thread exit, including a group leader exiting while other threads remain, must not stop collection for a live thread group. Before removing a process, confirm group completion and guard against PID reuse. Delayed or ambiguous events must trigger identity validation or reconciliation instead of removing a newer identity. Do not attempt a final resource snapshot after exit. If a process disappears before its start time can be read, preserve the lifecycle notification as unverified rather than invent a validated identity or measurements.

Discovery snapshots establish the inventory; they are separate from the resource snapshots collected by ADRs 0001 and 0002. The 30-second reconciliation interval does not set per-process or system resource sampling intervals. Neither kind of snapshot is an atomic view of the system.

### Connector availability and recovery

The connector requires target-kernel support (`CONFIG_CONNECTOR` and `CONFIG_PROC_EVENTS`), appropriate permissions, and a compatible namespace context. Check subscription success and report unavailable event capture instead of assuming that an open socket provides lifecycle coverage. Use the process connector ABI supported by the target kernel; newer optional event filtering must not be required for basic subscription.

Netlink notifications can be lost. Report receive overflows, reader failures, and detected event gaps; request an early reconciliation when loss is detected, coalescing recovery requests under the scan scheduling limit. Keep the regular 30-second reconciliation active even when no loss is reported. After reconnecting, subscribe before a recovery scan. A reconciliation scan repairs the current inventory but cannot reconstruct the full lifecycle of processes that started and exited between scans.

If the connector cannot be used, continue procfs discovery and routine resource sampling, report reduced lifecycle coverage, and retry the subscription with bounded backoff. Do not activate eBPF as a discovery fallback. A missed short-lived process may remain undiscovered in this degraded mode.

### Forensic observer mode

Enter `forensic-observer-mode` only after an anomaly is detected. Runtime coordination will receive an anomaly trigger identifying the affected process or thread and the reason for investigation. A qualifying trigger is required to activate eBPF, even when ADR-0002's adaptive sampling changes the collection interval. [ADR-0005: Proposed forensic trigger policy](0005-trigger-and-bound-forensic-capture.md)

Validate the target before starting a forensic session. Process targets use PID and start time; thread targets additionally require TID and thread start time, available from `/proc/<pid>/task/<tid>/stat`. Thread instrumentation belongs to forensic scope and does not require routine per-thread sampling in normal mode. Keep netlink discovery, 30-second reconciliation, and normal procfs collectors running while forensic sessions are active.

Use Aya to load and attach the eBPF programs required by the investigation. Filter collected data to the selected process or thread identities, with protection against PID/TID reuse. Consume forensic records through Aya's `RingBuf` and Tokio's `AsyncFd`, forwarding timestamped observations through the output path. This event path serves forensic analysis rather than replacing netlink discovery. Probe selection and the forensic record schema remain to be defined.

Forensic sessions will have configured capture duration and resource limits. Repeated triggers for an active target will be coalesced, and ending one session must not tear down instrumentation still needed by another. Stop a session when its capture policy completes or its target exits; release its target filters, attached programs, reader tasks, and buffers when no longer needed. Return to normal mode when all sessions end. Shutdown and partial initialization failures must also release acquired eBPF resources.

If eBPF support, permissions, required probes, or ring-buffer support are unavailable, report that forensic capture could not start and continue normal observation. Forensic event loss must be reported as incomplete capture. Detailed anomaly thresholds, probe sets, capture durations, resource budgets, and session completion rules require a subsequent design decision and validation on target devices.

## Rationale

Procfs establishes the initial inventory and periodically repairs it. Netlink provides prompt lifecycle notifications between scans without requiring eBPF for routine discovery. Reserving eBPF for anomaly-triggered sessions separates baseline observation from targeted investigation while retaining the same process identities and output pipeline.

## Consequences

### Positive

- Normal discovery works without eBPF support or loaded eBPF programs.
- New processes and exits can be detected between 30-second inventory scans.
- Reconciliation repairs stale state after missed notifications or temporary connector failure.
- Targeted forensic sessions can investigate anomalous processes or individual threads while routine observation continues.
- Resource sampling retains its independent adaptive schedules and shared concurrency limit.

### Negative

- Connector support, permissions, and namespace restrictions vary across deployments.
- Netlink event loss and scan-only operation can miss short-lived processes.
- Task events, PID reuse, and concurrent scans require careful identity and thread-group handling.
- Forensic sessions add eBPF compatibility requirements, resource use, and cleanup responsibilities.
- Targeted capture starts after detection and cannot recover eBPF observations from before activation.
- Anomaly and capture policies still need to be specified before forensic monitoring can be implemented.

## References

- [Linux kernel documentation: Kernel Connector](https://docs.kernel.org/driver-api/connector.html)
- [Linux process connector event ABI](https://github.com/torvalds/linux/blob/master/include/uapi/linux/cn_proc.h)
- [Linux process connector implementation](https://github.com/torvalds/linux/blob/master/drivers/connector/cn_proc.c)
- [Linux connector configuration](https://github.com/torvalds/linux/blob/master/drivers/connector/Kconfig)
- [Linux kernel documentation: BPF ring buffer](https://docs.kernel.org/bpf/ringbuf.html)
- [Aya ring buffer](https://docs.rs/aya/latest/aya/maps/ring_buf/struct.RingBuf.html)
- [ADR-0002: Schedule Per-Process Sampling Workers](0002-schedule-per-process-sampling-workers.md)
