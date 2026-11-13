# Comparison of RDK Components with rdk-observer-rs on ARM Ready QEMU

## Test environment

- architecture: AArch64 GNU/Linux;
- operating system: RDK 6.1 (kirkstone);
- Linux kernel: 5.15.162;
- CPU: 1 virtual Cortex-A53;
- RAM: 996,908 KiB;
- `/proc/<pid>/io`: available;
- `/proc/<pid>/smaps`: available;
- `/proc/<pid>/smaps_rollup`: available.

## Metric coverage

| Category | rdk-observer-rs | CPUProcAnalyzer | MemInsight |
|---|---|---|---|
| System CPU | ✅ | ✅ | ✅ |
| Process CPU | ✅ | ✅ | <span style="color:#d4a000">CPU_TIME</span> |
| System memory | ✅ | ✅ | ✅ |
| Process memory | ✅ | ✅ | ✅ |
| Process I/O | ✅ | ❌ | ❌ |
| System storage I/O | ✅ | ❌ | ❌ |
| System network | <span style="color:#d4a000">In Progress</span> | ❌ | ❌ |
| System pressure | <span style="color:#d4a000">In Progress</span> | ❌ | ❌ |
| Netlink process lifecycle | ✅ Fork/Exec/Comm/Exit | <span style="color:#d4a000">EXEC snapshot only</span> | ❌ |
| Short-lived process discovery | ✅ | <span style="color:#d4a000">Partial</span> | ❌ |
| PID reuse-safe identity | ✅ | ❌ | ❌ |
| Adaptive collection intervals | ✅ | ❌ | ❌ |
| Persistent output | <span style="color:#d4a000">In Progress</span> | Text files | CSV/JSON/T2 |

## Functional differences

### rdk-observer-rs

- unified runtime for system and process CPU, memory, process I/O, and
  block-device I/O monitoring;
- netlink lifecycle tracking with periodic `/proc` reconciliation, including
  detection of short-lived processes;
- PID and start-time identity to prevent incorrect attribution after PID reuse;
- raw counters together with derived CPU utilization and I/O rates;
- separate system, initial-process, and ongoing-process intervals with sampling
  jitter and bounded concurrency;


### CPUProcAnalyzer

- collects CPU, memory, file-descriptor, thread, load-average, and RDK client
  statistics;
- monitors configured process names through helper commands, while dynamic mode
  produces only a one-time snapshot on `EXEC`.

### MemInsight

- collects detailed process and system memory, page faults, fragmentation, and
  optional DDR bandwidth;
- uses periodic `/proc` scans and writes CSV, JSON, or T2 reports.

## Resource usage benchmark methodology
Each component was run separately for 60 seconds with a one-second metric
collection interval. For `rdk-observer-rs`, this interval was used only for the
comparison and is not a production configuration.

### rdk-observer-rs

```text
rdk-observer \
    --system-collection-interval 1 \
    --process-collection-interval-initial 1 \
    --process-collection-interval-ongoing 1
```

### CPUProcAnalyzer

```text
SleepInterval = 1
TimeToRun = 0
DynamicProcess = 0
MonitorAllProcess = 0
MemoryLimit = 65536
```

### MemInsight

```text
meminsight --iterations 700 --interval 1 --fmt csv --output <directory>
```

### Resource measurement

```text
perf stat -p <pid> \
    -e task-clock,context-switches,cpu-migrations,page-faults,minor-faults,major-faults \
    -- sleep 60
```

CPU is reported as a share of one vCPU.

RSS, HWM, and thread counts were read from `/proc`.

## Resource usage

The results in this table were collected with a metric collection interval of <u><strong>1 second</strong></u>.

| Component | CPU | CPU task time | RSS | HWM | Threads |
|---|---:|---:|---:|---:|---:|
| rdk-observer-rs | 24.2% | 14,525.53 ms | 2,384 KiB | 2,420 KiB | 10 |
| CPUProcAnalyzer | 69.2% | 41,553.42 ms | 7,224 KiB | 9,128 KiB | 3 |
| MemInsight | 4.7% | 2,819.56 ms | 1,612 KiB | 1,764 KiB | 1 |

CPU usage by collection interval:

| Interval | rdk-observer-rs CPU | CPUProcAnalyzer CPU | MemInsight CPU |
|---:|---:|---:|---:|
| 1 s | 24.2% | 69.2% | 4.7% |
| 2 s | 13.5% | 75.9% | 2.4% |
| 5 s | 7.7% | 70.8% | 0.9% |
| 10 s | 4.9% | 66.5% | 0.7% |

```console
# rdk-observer-rs, PID 292691
$ grep -E '^(VmRSS|VmHWM|Threads):' /proc/292691/status
VmHWM:     2420 kB
VmRSS:     2384 kB
Threads:   10
$ perf stat -x, -o /tmp/rdk-observer-perf.csv -p 292691 -e task-clock,context-switches,cpu-migrations,page-faults,minor-faults,major-faults -- sleep 60
$ grep 'task-clock' /tmp/rdk-observer-perf.csv
14525.53,msec,task-clock,14525531872,100.00,0.242,CPUs utilized

# CPUProcAnalyzer, PID 293301
$ grep -E '^(VmRSS|VmHWM|Threads):' /proc/293301/status
VmHWM:     9128 kB
VmRSS:     7224 kB
Threads:   3
$ perf stat -x, -o /tmp/cpuprocanalyzer-perf.csv -p 293301 -e task-clock,context-switches,cpu-migrations,page-faults,minor-faults,major-faults -- sleep 60
$ grep 'task-clock' /tmp/cpuprocanalyzer-perf.csv
41553.42,msec,task-clock,41553417072,100.00,0.692,CPUs utilized

# MemInsight, PID 295784
$ grep -E '^(VmRSS|VmHWM|Threads):' /proc/295784/status
VmHWM:     1764 kB
VmRSS:     1612 kB
Threads:   1
$ perf stat -x, -o /tmp/meminsight-perf.csv -p 295784 -e task-clock,context-switches,cpu-migrations,page-faults,minor-faults,major-faults -- sleep 60
$ grep 'task-clock' /tmp/meminsight-perf.csv
2819.56,msec,task-clock,2819558704,100.00,0.047,CPUs utilized
```

### CPU

CPUProcAnalyzer used 69.2% of one vCPU, `rdk-observer-rs` used 24.2%, and
MemInsight used 4.7%. For `rdk-observer-rs`, the one-second interval was used
only for comparison with the other components.

### RAM

RSS was 2,384 KiB for `rdk-observer-rs`, 7,224 KiB for CPUProcAnalyzer, and
1,612 KiB for MemInsight. HWM was 2,420, 9,128, and 1,764 KiB, respectively.

### Threads

MemInsight used one thread, CPUProcAnalyzer used three, and `rdk-observer-rs`
used ten. `rdk-observer-rs` uses a Tokio runtime and blocking pool.

## Context switches and page faults

| Component | Context switches | Page faults | Major faults |
|---|---:|---:|---:|
| rdk-observer-rs | 64,079 | 1,842 | 0 |
| CPUProcAnalyzer | 22,845 | 169,820 | 0 |
| MemInsight | 1,361 | 2,486 | 0 |

```console
# rdk-observer-rs
$ grep -E 'context-switches|page-faults|major-faults' /tmp/rdk-observer-perf.csv
64079,,context-switches,14525531872,100.00,4.411,K/sec
1842,,page-faults,14525531872,100.00,126.811,/sec
0,,major-faults,14525531872,100.00,0.000,/sec

# CPUProcAnalyzer
$ grep -E 'context-switches|page-faults|major-faults' /tmp/cpuprocanalyzer-perf.csv
22845,,context-switches,41553417072,100.00,549.774,/sec
169820,,page-faults,41553417072,100.00,4.087,K/sec
0,,major-faults,41553417072,100.00,0.000,/sec

# MemInsight
$ grep -E 'context-switches|page-faults|major-faults' /tmp/meminsight-perf.csv
1361,,context-switches,2819558704,100.00,482.700,/sec
2486,,page-faults,2819558704,100.00,881.698,/sec
0,,major-faults,2819558704,100.00,0.000,/sec
```

## Stored data

| Component | File count | Total size |
|---|---:|---:|
| rdk-observer-rs | 0 | 0 bytes |
| CPUProcAnalyzer | 244 | 43,119 bytes |
| MemInsight | 63 | 279,691 bytes |

```console
# rdk-observer-rs
$ find /tmp/rdk-observer-output -type f 2>/dev/null | wc -l
0
$ find /tmp/rdk-observer-output -type f -exec cat {} + 2>/dev/null | wc -c
0

# CPUProcAnalyzer
$ find /tmp/cpuprocanalyzer -type f | wc -l
244
$ find /tmp/cpuprocanalyzer -type f -exec cat {} + | wc -c
43119

# MemInsight
$ find /tmp/meminsight-output -type f | wc -l
63
$ find /tmp/meminsight-output -type f -exec cat {} + | wc -c
279691
```

rdk-observer-rs updates records in the process registry but does not save them
to persistent output.
