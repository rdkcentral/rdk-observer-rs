# ADR-0004: Use CBOR for Observation Records

## Status

Proposed

## Context

The observer will send many process and system snapshots to a backend. Encoding and decoding each record consumes CPU time, and the combined payloads consume network bandwidth. These costs matter on resource-constrained devices and when record volume increases.

The sender and backend need an agreed format that preserves the types of record fields and can evolve as new measurements are added.

## Decision

Encode observation records sent to the backend using CBOR, as defined by RFC 8949. The Rust observer will serialize records before transmission, and the backend will decode them using an agreed, versioned record schema. The transport will carry CBOR as binary data; message boundaries and batching will be defined with the transport protocol.

Keep the schema explicit: define field names or identifiers, units, optional fields, and versioning rules. Compare CBOR with a text-based baseline using representative process and system records. Measure encoded size, serialization time, deserialization time, and CPU usage at expected record rates on target devices and the backend.

## Rationale

CBOR is a compact binary format designed for relatively small messages and efficient implementations. It can avoid repeated textual representation of values across a high volume of records and may reduce transfer and processing costs. Actual savings depend on the schema, library, transport, and workload, so performance improvements must be measured rather than assumed.

## Consequences

### Positive

- Smaller encoded records may reduce bandwidth and buffering needs when many records are transmitted.
- Binary values can be represented without converting every field to text.
- A standard format allows independent Rust and backend implementations.

### Negative

- CBOR is not human-readable without a decoder, which makes inspecting raw traffic harder.
- The backend must support the same schema and handle unknown or optional fields as the schema evolves.
- CBOR does not guarantee lower CPU cost or smaller payloads for every record; representative benchmarks are needed.

## References

- [RFC 8949: Concise Binary Object Representation (CBOR)](https://www.rfc-editor.org/rfc/rfc8949.html)
