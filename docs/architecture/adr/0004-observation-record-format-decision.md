# ADR-0004: Observation Record Format Decision

## Status

Accepted

## Context

The observer will send many process and system snapshots to a backend. Encoding and decoding each record consumes CPU time, and the combined payloads consume network bandwidth. These costs matter on resource-constrained devices and when record volume increases.

The sender and backend need an agreed format that preserves the types of record fields and can evolve as new measurements are added.

JSON, CBOR, and Protocol Buffers (Protobuf) were compared using the small and medium record cases below. The Protobuf measurements use `prost`, the Rust Protobuf implementation; `prost` in the tables refers to Protobuf encoding.

## Decision

Encode observation records sent to the backend using Protocol Buffers. Use `prost` to encode and decode records in the Rust observer, with Rust message types generated from shared `.proto` definitions using `prost-build`. The backend will decode records using bindings generated from the same schema. The transport will carry Protobuf as binary data; message boundaries and batching will be defined with the transport protocol.

Keep the schema explicit: define field numbers, types, units, field presence, and versioning rules. Preserve existing field numbers and reserve the numbers and names of removed fields. Use explicit presence for measurements where an unavailable value must be distinguished from zero.

## Rationale

Protobuf produces the smallest payloads and the lowest serialization and deserialization times in every measured case below. Its encoded sizes are 32.2% to 39.7% of JSON and 47.6% to 58.9% of CBOR. These results support choosing Protobuf to reduce bandwidth, buffering, and encoding and decoding time for observation records.

For example, Medium x100000 uses 10.47 MiB with Protobuf, compared with 27.18 MiB for JSON and 17.77 MiB for CBOR. Protobuf serialization takes 14.24 ms and deserialization takes 45.37 ms, compared with 38.77 ms and 95.68 ms for JSON, and 25.03 ms and 83.70 ms for CBOR.

Shared `.proto` definitions also provide an explicit contract between the observer and backend. The results describe the measured workloads; benchmark hardware, library versions, record definitions, and methodology are not recorded here. Validate performance and CPU usage on target devices and the backend at expected record rates before deployment.

### Encoded size comparison

The following measurements compare JSON, CBOR, and Protobuf payload sizes, with Protobuf encoded using `prost`. Percentage columns show the encoded size of the first format relative to the second format, so lower values indicate smaller payloads.

| Case           |       JSON |       CBOR | CBOR vs JSON |      prost | prost vs JSON | prost vs CBOR |
|----------------|-----------:|-----------:|-------------:|-----------:|--------------:|--------------:|
| Small x1       |       54 B |       38 B |        70.4% |       19 B |         35.2% |         50.0% |
| Small x10      |      553 B |      374 B |        67.6% |      178 B |         32.2% |         47.6% |
| Small x100     |   5.49 KiB |   3.78 KiB |        68.8% |   1.96 KiB |         35.6% |         51.8% |
| Small x1000    |  56.59 KiB |  39.45 KiB |        69.7% |  21.13 KiB |         37.3% |         53.6% |
| Small x10000   | 585.80 KiB | 407.08 KiB |        69.5% | 222.49 KiB |         38.0% |         54.7% |
| Small x100000  |   5.91 MiB |   4.14 MiB |        70.0% |   2.35 MiB |         39.7% |         56.7% |
| Medium x1      |      261 B |      170 B |        65.1% |       94 B |         36.0% |         55.3% |
| Medium x10     |   2.75 KiB |   1.78 KiB |        64.6% |   1.03 KiB |         37.5% |         58.1% |
| Medium x100    |  27.14 KiB |  17.48 KiB |        64.4% |  10.03 KiB |         37.0% |         57.4% |
| Medium x1000   | 274.17 KiB | 177.46 KiB |        64.7% | 102.92 KiB |         37.5% |         58.0% |
| Medium x10000  |   2.70 MiB |   1.75 MiB |        65.0% |   1.03 MiB |         38.0% |         58.5% |
| Medium x100000 |  27.18 MiB |  17.77 MiB |        65.4% |  10.47 MiB |         38.5% |         58.9% |

### Processing time comparison

The following measurements compare serialization (`ser`) and deserialization (`de`) times for the same cases. Lower values indicate faster processing.

| Case           |  JSON ser |   JSON de |  CBOR ser |   CBOR de | prost ser |  prost de |
|----------------|----------:|----------:|----------:|----------:|----------:|----------:|
| Small x1       |     83 ns |    112 ns |    139 ns |    212 ns |     31 ns |     44 ns |
| Small x10      |    951 ns |   1.36 µs |    669 ns |   1.92 µs |    237 ns |    802 ns |
| Small x100     |   7.89 µs |  13.27 µs |   5.12 µs |  17.95 µs |   2.61 µs |   7.24 µs |
| Small x1000    |  81.77 µs | 135.73 µs |  49.36 µs | 178.03 µs |  26.45 µs |  70.18 µs |
| Small x10000   | 802.00 µs |   1.36 ms | 459.03 µs |   1.79 ms | 261.48 µs | 692.29 µs |
| Small x100000  |   8.75 ms |  14.69 ms |   5.35 ms |  18.86 ms |   2.68 ms |   7.67 ms |
| Medium x1      |    372 ns |    681 ns |    337 ns |    765 ns |     92 ns |    323 ns |
| Medium x10     |   3.41 µs |   8.56 µs |   2.33 µs |   8.29 µs |   1.15 µs |   4.35 µs |
| Medium x100    |  39.23 µs |  88.34 µs |  22.00 µs |  78.32 µs |  11.46 µs |  41.81 µs |
| Medium x1000   | 373.59 µs | 928.67 µs | 222.06 µs | 809.00 µs | 128.24 µs | 435.81 µs |
| Medium x10000  |   4.13 ms |   9.28 ms |   2.26 ms |   8.18 ms |   1.44 ms |   4.44 ms |
| Medium x100000 |  38.77 ms |  95.68 ms |  25.03 ms |  83.70 ms |  14.24 ms |  45.37 ms |

## Consequences

### Positive

- The measured Protobuf payloads are smaller than JSON and CBOR, reducing payload bandwidth and buffering needs for these cases.
- The measured serialization and deserialization times are lower than JSON and CBOR for every case.
- Shared `.proto` definitions support generated Rust and backend bindings with an explicit, typed record contract.

### Negative

- Protobuf payloads require a decoder and the corresponding schema for meaningful inspection.
- Schema generation adds a build step and requires maintaining shared `.proto` definitions and generation tooling.
- Schema changes must preserve wire compatibility and define how absent measurements and default values are interpreted.
- Performance depends on the schema, implementation, and workload; the recorded results must be validated on target devices.

## References

- [Protocol Buffers overview](https://protobuf.dev/overview/)
- [Protocol Buffers language guide (proto3)](https://protobuf.dev/programming-guides/proto3/)
- [prost: Protocol Buffers implementation for Rust](https://docs.rs/prost/latest/prost/)
- [prost-build: Rust code generation from Protobuf definitions](https://docs.rs/prost-build/latest/prost_build/)
