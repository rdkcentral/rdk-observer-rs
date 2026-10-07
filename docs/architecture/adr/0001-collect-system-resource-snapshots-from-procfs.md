# ADR-0001: Collect System Resource Snapshots from procfs

## Status

Proposed

## Context

The observer needs system-wide snapshots of DRAM, CPU, and storage activity alongside its process records. On Linux, procfs exposes these measurements through kernel-maintained files without requiring a separate monitoring service.

A snapshot is a set of readings collected during one sampling pass, not an atomic view of the entire system. Some readings are current values, while others are counters that must be compared across samples.

## Decision

Read the following procfs files during each sampling pass:

| Resource | Source | Snapshot data |
| --- | --- | --- |
| DRAM | `/proc/meminfo` | `MemTotal` and `MemAvailable`, in the units reported by the file. |
| CPU | `/proc/stat` | Aggregate `cpu` time counters; preserve the raw counters for comparison with the next sample. |
| Storage I/O | `/proc/diskstats` | Per-device read and write counters, identified by major/minor device number and device name. |

Timestamp each sampling pass and retain the source measurements. Calculate CPU utilization and storage I/O rates only from two valid samples with elapsed time between them. Treat the first sample as a baseline, and discard a delta if a counter resets or a device disappears or changes identity.

Storage here means block-device I/O activity. `/proc/diskstats` does not provide filesystem capacity or free space; those measurements require a filesystem API such as `statvfs` and are outside this decision.

## Rationale

These procfs files provide the system-level values needed for periodic observation using a small number of reads. Keeping raw counters makes derived rates reproducible and avoids presenting cumulative CPU or disk activity as an instantaneous percentage or rate.

System snapshots also provide the device-wide baseline needed to interpret and normalize per-PID observations. Aggregate CPU deltas provide context for per-PID CPU time, `MemTotal` and `MemAvailable` provide context for process memory usage, and block-device counters provide a system-level reference for process I/O. Comparing both views helps identify resource usage that cannot be attributed to a monitored process, including kernel activity and work performed by processes outside the selected set.

This system-wide path complements rather than replaces per-PID sampling. It can run at a lower, independently configured frequency because it establishes overall context, while targeted per-PID workers can collect selected process metrics more frequently. The observations are correlated by timestamp; they are not expected to form an atomic snapshot or sum exactly because the sources are read at different times and expose different accounting boundaries.

## Consequences

### Positive

- A single sampling path can collect system-wide DRAM, CPU, and storage I/O readings.
- The same source data can be used to derive rates over different sampling intervals.
- System-wide baselines can normalize and provide context for per-PID observations.
- Differences between system and per-PID measurements can expose kernel, unmonitored, or otherwise unattributed resource usage.

### Negative

- The readings are collected sequentially, so a snapshot is not atomic.
- CPU utilization and I/O rates are unavailable until a second valid sample exists.
- Device selection needs care to avoid double-counting whole disks and their partitions.
- Filesystem capacity and free space need a separate source if added later.

## References

- [Linux kernel documentation: procfs](https://docs.kernel.org/filesystems/proc.html)
- [Linux kernel documentation: I/O statistics fields](https://docs.kernel.org/admin-guide/iostats.html)
