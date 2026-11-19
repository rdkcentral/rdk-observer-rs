# Comparison of rdk-observer-rs with the Existing RDK Monitoring Stack

## Overview

`rdk-observer-rs` is an event-driven system monitor. It reacts to process fork,
exec, comm, and exit events as they happen, updates its process registry, and
collects per-process CPU, memory, and I/O metrics. In parallel, it collects
system-wide CPU, memory, and block-device I/O metrics. A periodic `/proc`
reconciliation complements event tracking.

CPUProcAnalyzer and MemInsight are two snapshot-based specialized components.
CPUProcAnalyzer performs process discovery and metric collection through shell
commands. It periodically checks configured process names and collects CPU and
basic resource usage. MemInsight periodically scans `/proc` and creates detailed
system and process memory snapshots. They overlap for some CPU and memory
metrics but otherwise serve different purposes.

## Metric coverage

| Category | rdk-observer-rs | CPUProcAnalyzer + MemInsight |
|---|---|---|
| System CPU | ✅ | ✅ |
| Process CPU | ✅ | ✅ |
| System memory | ✅ | ✅ |
| Process memory | ✅ | ✅ |
| Process I/O | ✅ | ❌ |
| System storage I/O | ✅ | ❌ |
| System network | <span style="color:#d4a000">In Progress</span> | ❌ |
| System pressure | <span style="color:#d4a000">In Progress</span> | ❌ |
| Event-based collection | ✅ | <span style="color:#d4a000">EXEC snapshot only</span> |
| Netlink process lifecycle | ✅ Fork/Exec/Comm/Exit | <span style="color:#d4a000">EXEC snapshot only</span> |
| Short-lived process discovery | ✅ | <span style="color:#d4a000">Partial</span> |
| PID reuse-safe identity | ✅ | ❌ |
| Adaptive collection intervals | ✅ | ❌ |
| Output | <span style="color:#d4a000">Protobuf binary records (In Progress)</span> | ✅ Text/CSV/JSON/T2 |

## Functional differences

### rdk-observer-rs

- provides system and process CPU, memory, process I/O, and block-device I/O in
  one runtime;
- collects metrics directly in-process without invoking shell commands;
- reacts to fork, exec, comm, and exit events through netlink instead of waiting
  for the next periodic scan;
- reconciles the event-driven process registry with `/proc` periodically;
- detects short-lived processes and protects against incorrect attribution
  after PID reuse;
- supports separate system, initial-process, and ongoing-process collection
  intervals.

### CPUProcAnalyzer + MemInsight

- CPUProcAnalyzer uses shell commands for process discovery and metric
  collection, and provides file-descriptor counts, load-average, RDK
  client-specific statistics, and process and thread text output;
- MemInsight provides detailed `smaps` fields, page-fault reports, and CSV,
  JSON, or T2 output.

## Resource usage

The results in this table were collected with a metric collection interval of
<u><strong>1 second</strong></u>.

| Component | CPU | CPU time | Peak memory usage | Processes, max | Threads, max |
|---|---:|---:|---:|---:|---:|
| rdk-observer-rs | 38.6% | 23,149.46 ms | 1,648 KiB | 1 | 10 |
| CPUProcAnalyzer + MemInsight | 73.9% | 44,372.46 ms | 7,764 KiB | 6 | 11 |

CPU usage by collection interval:

| Collection interval | rdk-observer-rs CPU | CPUProcAnalyzer + MemInsight CPU |
|---:|---:|---:|
| 1 s | 38.6% | 73.9% |
| 2 s | 23.1% | 77.4% |
| 5 s | 12.5% | 67.5% |
| 10 s | 7.1% | 65.9% |

### CPU

At one second, `rdk-observer-rs` used approximately 1.9 times less CPU. At ten
seconds, it used approximately 9.3 times less CPU.

### RAM

The kernel-recorded cgroup peak was 1,648 KiB for `rdk-observer-rs` and 7,764
KiB for CPUProcAnalyzer and MemInsight together. 

### Processes and threads

`rdk-observer-rs` stayed at 1 process and 10 threads throughout the test. The 10
threads are 1 main thread and 9 Tokio runtime or blocking workers.

CPUProcAnalyzer normally runs as 1 process with 4 threads, while MemInsight runs
as 1 process with 1 thread. Short-lived shell and helper processes increased the
highest captured values to 6 processes and 11 threads.

## Context switches and page faults

| Component | Context switches | Page faults | Major page faults |
|---|---:|---:|---:|
| rdk-observer-rs | 129,361 | 2,509 | 0 |
| CPUProcAnalyzer + MemInsight | 32,769 | 166,479 | 0 |

## Test environment

- architecture: AArch64 GNU/Linux;
- operating system: RDK 6.1 (kirkstone);
- Linux kernel: 5.15.162;
- CPU: 1 virtual Cortex-A53;
- RAM: 996,908 KiB;
- swap: disabled;
- cgroup v1 CPU, memory, pids, and perf-event controllers: available;
- `/proc/<pid>/io`: available;
- `/proc/<pid>/smaps_rollup`: available.

## Resource usage benchmark methodology

Each option was measured at collection intervals of 1, 2, 5, and 10
seconds. 

CPUProcAnalyzer and MemInsight were placed in the same cgroup. All their
processes, threads, and inherited helper processes were therefore included in
the result. `rdk-observer-rs` was measured in a separate cgroup using the same
method.

### Existing stack

CPUProcAnalyzer:

```text
SleepInterval = <interval>
TimeToRun = 0
DynamicProcess = 1
MonitorAllProcess = 0
MemoryLimit = 65536
```

MemInsight:

```text
meminsight \
    --iterations 700 \
    --interval <interval> \
    --fmt csv \
    --output <meminsight-directory>
```

### rdk-observer-rs

```text
rdk-observer --system-collection-interval <interval> \
    --process-collection-interval-initial <interval> \
    --process-collection-interval-ongoing <interval>
```

### Resource measurement

CPU, scheduler events, peak memory, process count, and thread count were
measured for the complete cgroup.

```text
perf stat -x, -a -G /<cgroup> \
    -e task-clock,context-switches,cpu-migrations,page-faults,minor-faults,major-faults \
    -- sleep 60

cat /sys/fs/cgroup/memory/<cgroup>/memory.max_usage_in_bytes
wc -l < /sys/fs/cgroup/pids/<cgroup>/cgroup.procs
wc -l < /sys/fs/cgroup/pids/<cgroup>/tasks
```

## Benchmark command outputs

The following output is from the one-second runs.

### Existing stack

```console
$ echo 0 > /sys/fs/cgroup/memory/rdk-bench-existing-stack-1-366761/memory.max_usage_in_bytes
$ perf stat -x, -a -G /rdk-bench-existing-stack-1-366761 \
    -e task-clock,context-switches,cpu-migrations,page-faults,minor-faults,major-faults \
    -- sleep 60
44372.46,msec,task-clock,/rdk-bench-existing-stack-1-366761,44378536656,100.00,0.739,CPUs utilized
32769,,context-switches,/rdk-bench-existing-stack-1-366761,44378536656,100.00,738.499,/sec
0,,cpu-migrations,/rdk-bench-existing-stack-1-366761,44378536656,100.00,0.000,/sec
166479,,page-faults,/rdk-bench-existing-stack-1-366761,44378536656,100.00,3.752,K/sec
166479,,minor-faults,/rdk-bench-existing-stack-1-366761,44378536656,100.00,3.752,K/sec
0,,major-faults,/rdk-bench-existing-stack-1-366761,44378536656,100.00,0.000,/sec

$ cat /sys/fs/cgroup/memory/rdk-bench-existing-stack-1-366761/memory.max_usage_in_bytes
7950336
$ wc -l < /sys/fs/cgroup/pids/rdk-bench-existing-stack-1-366761/cgroup.procs
6
$ wc -l < /sys/fs/cgroup/pids/rdk-bench-existing-stack-1-366761/tasks
11
```

### rdk-observer-rs

```console
$ echo 0 > /sys/fs/cgroup/memory/rdk-bench-rdk-observer-1-369146/memory.max_usage_in_bytes
$ perf stat -x, -a -G /rdk-bench-rdk-observer-1-369146 \
    -e task-clock,context-switches,cpu-migrations,page-faults,minor-faults,major-faults \
    -- sleep 60
23149.46,msec,task-clock,/rdk-bench-rdk-observer-1-369146,23154394464,100.00,0.386,CPUs utilized
129361,,context-switches,/rdk-bench-rdk-observer-1-369146,23154394464,100.00,5.588,K/sec
0,,cpu-migrations,/rdk-bench-rdk-observer-1-369146,23154394464,100.00,0.000,/sec
2509,,page-faults,/rdk-bench-rdk-observer-1-369146,23154394464,100.00,108.383,/sec
2509,,minor-faults,/rdk-bench-rdk-observer-1-369146,23154394464,100.00,108.383,/sec
0,,major-faults,/rdk-bench-rdk-observer-1-369146,23154394464,100.00,0.000,/sec

$ cat /sys/fs/cgroup/memory/rdk-bench-rdk-observer-1-369146/memory.max_usage_in_bytes
1687552
$ wc -l < /sys/fs/cgroup/pids/rdk-bench-rdk-observer-1-369146/cgroup.procs
1
$ wc -l < /sys/fs/cgroup/pids/rdk-bench-rdk-observer-1-369146/tasks
10
```
