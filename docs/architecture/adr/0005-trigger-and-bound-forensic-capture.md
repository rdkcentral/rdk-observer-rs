# ADR-0005: Trigger and Bound Forensic Capture

## Status

Proposed

## Context

Normal observation uses netlink lifecycle events and periodic procfs measurements to track processes and resource usage. A CPU spike, rapid memory growth, or repeated restart can identify a component that warrants investigation. Diagnosing the cause may require thread-level scheduler events, page faults, or other details that routine snapshots do not capture. On a resource-constrained gateway, that extra collection needs a bounded cost.

ADR-0003 reserves eBPF for anomaly-triggered capture and defines how it coexists with normal discovery and sampling. This ADR defines the proposed trigger policy, target selection, and session boundaries. [ADR-0003](0003-trigger-process-capture-with-bpf-events.md)

## Decision

### Decide when to capture

Evaluate the proposed rules against routine process and system observations and lifecycle events. A capture request identifies the rule, the evidence that matched it, and the process or thread to investigate. A rule requires valid observations for its time window. Missing samples, invalid counter deltas, and a changed process identity can leave a window incomplete. Evaluate one-second bucket rules only where the sampling cadence supports them. Adaptive sampling can improve the evidence available to the detector while eBPF activation remains tied to a qualifying trigger. [ADR-0002](0002-schedule-per-process-sampling-workers.md)

Use sustained or recurring evidence for CPU, memory, scheduler, storage, and page-fault anomalies (FT-1 through FT-3 and FT-7 through FT-9). Pressure gates on selected rules connect a process-level symptom to device-level contention. FT-7 depends on bounded pre-trigger thread measurements for watched processes. FT-9 uses process fault counters to qualify the trigger and attributes faults to threads during capture.

Use component restart history for FT-4 and a fatal exit of a configured critical component for FT-5. FT-5 can open a session only when the component restarts within its proposed window. A per-label cooldown and suppression while FT-4 is active limit repeated captures of a single crash loop. FT-6 pairs a rise in the system OOM-kill counter with a tracked process exiting on `SIGKILL`. That correlation is evidence of a possible OOM kill. A repeat can select a live process with growing anonymous RSS for investigation.

Use system pressure and available memory as corroboration for eligible process-level rules (FT-10). System hardware-health signals are recorded for diagnosis (FT-11). Both entries are targetless. A process-attributable hardware fault delivered as a fatal signal can qualify under FT-5. The README records the proposed thresholds, time windows, and review notes. [Forensic trigger events](../../../README.md#forensic-trigger-events)

### Resolve the target and bound the session

1. Verify that the target is live and validate its PID and start time. For a thread, also validate its TID and thread start time. For a rule that investigates a restart, use `logical_label` to resolve the next instance and validate the new identity before attaching. This keeps measurement histories and captures tied to the right instance after PID or TID reuse.
2. Have `runtime` coordinate sessions and coalesce repeated requests for an active target. Apply configured duration and resource limits during capture. Keep netlink discovery, procfs reconciliation, and normal resource sampling running throughout the session.
3. Use Aya to attach the eBPF probes needed for the investigation and scope records to the validated target. Read timestamped records through Aya's `RingBuf` and Tokio's `AsyncFd` and forward them to the observation output path. Probe selection and the forensic record schema remain open.
4. End the session when its capture policy completes or its target exits. Release target filters, reader tasks, buffers, and probe attachments when their last session is finished. Return to normal mode after the last session ends. On shutdown or partial startup failure, clean up acquired resources. Report unavailable capture and event loss while normal observation continues.

## Rationale

Starting capture only after a qualifying event limits tracing overhead to a defined target and time window. The routine samples and lifecycle events establish what led to the trigger. The forensic trace adds detail about the live process or thread after attachment.

Different symptoms need different evidence. Sustained and episodic rules use history to distinguish persistent load from brief activity. A fatal exit can be significant on its own, while a live replacement is needed to collect a trace. System-wide pressure helps establish the severity of a symptom, and process-level evidence supplies the attribution needed to attach probes.

## Consequences

- Capture coverage depends on the availability and cadence of trigger inputs. Short-lived activity can end before capture starts. Lifecycle event loss can also hide restarts.
- Normal collection continues during tracing, so the combined CPU, memory, and output load must be measured on target devices.
- Kernel support, permissions, and probe availability can vary across RDK platforms. Session startup and capture completeness need to be reported so the output can be interpreted correctly.
- Before implementation is finalized, validate the proposed rules and thresholds on representative workloads and define supported probes, duration and resource limits, concurrent session limits, and the forensic record schema.
