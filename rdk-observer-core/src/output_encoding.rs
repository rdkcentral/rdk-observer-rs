//! Observation record encoding.
//!
//! [`proto`] contains the observation messages generated from the shared schema.
//! This module will map records from `output_model` to those messages and
//! encode/decode them using `prost`.
//! Conversions will preserve process identities, sampling timestamps, optional
//! measurements, and collections of thread identities, metadata, and measurements.
//! Unavailable values will remain distinguishable from measured zero.
//! The delivery protocol will define record framing and batching.

/// Protobuf messages generated from the `rdk.observer.v1` schema.
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/rdk.observer.v1.rs"));
}
