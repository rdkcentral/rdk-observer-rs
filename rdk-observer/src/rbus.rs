//! RBus provider exposing TR-181 `Device.DeviceInfo.ProcessStatus.` data model.

#![allow(non_camel_case_types)]

use anyhow::Context;
use rbus_core::RBusError;
use rbus_provider::element::RBusProviderElement;
use rbus_provider::element::object::rbus_object;
use rbus_provider::element::property::{RBusProviderGetter, RBusProviderGetterArgs, rbus_property};
use rbus_provider::element::table::{RBusProviderTableSync, RBusProviderTableSyncArgs, rbus_table};
use rbus_provider::provider::RBusProvider;
use rdk_observer_core::process_registry::ProcessRegistry;
use std::ffi::CStr;
use tracing::{info, instrument};

////////////////////////////////////////////////////////////////////////////////
/// RBus component name registered on the bus
const COMPONENT_NAME: &CStr = c"ProcessObserver";

////////////////////////////////////////////////////////////////////////////////
/// Connection to RBus component
pub struct RBusConnection {
    _handle: RBusProvider,
}

impl RBusConnection {
    ////////////////////////////////////////////////////////////////////////////////
    #[instrument(name = "rbus_open", skip_all)]
    pub fn open(registry: ProcessRegistry) -> anyhow::Result<Self> {
        let handle = RBusProvider::open(COMPONENT_NAME, move || Self::build_model(registry))
            .with_context(|| {
                format!("failed to register RBUS provider for component {COMPONENT_NAME:?}")
            })?;

        info!("RBus elements successfully registered");
        Ok(Self { _handle: handle })
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[rustfmt::skip]
    fn build_model(registry: ProcessRegistry) -> impl RBusProviderElement {
        let handler = move || RBus_ProcessStatus(registry.clone());
        rbus_object("Device", (
            rbus_object("DeviceInfo", (
                rbus_object("ProcessStatus", (
                    rbus_property("ProcessNumberOfEntries", handler()),
                    rbus_table("Process", handler(), (
                        rbus_property("PID", handler()),
                        rbus_property("Command", handler()),
                        rbus_property("Size", handler()),
                        rbus_property("CPUTime", handler()),
                        rbus_property("X_RDK_Timestamp", handler()),
                    )),
                )),
            )),
        ))
    }
}

///
/// Device.DeviceInfo.ProcessStatus.
/// - ProcessNumberOfEntries
///
/// Device.DeviceInfo.ProcessStatus.Process.{i}.
/// - PID
/// - Command
/// - Size
/// - CPUTime
///
struct RBus_ProcessStatus(ProcessRegistry);

impl RBusProviderTableSync for RBus_ProcessStatus {
    type UserData = ();

    ////////////////////////////////////////////////////////////////////////////////
    fn len(&mut self, _a: RBusProviderTableSyncArgs<Self::UserData>) -> Result<u32, RBusError> {
        Ok(self.0.get_snapshot().len() as u32)
    }
}

impl RBusProviderGetter for RBus_ProcessStatus {
    type UserData = ();

    ////////////////////////////////////////////////////////////////////////////////
    fn get(&mut self, args: RBusProviderGetterArgs<Self::UserData>) -> Result<(), RBusError> {
        let name = &**args.path_name;
        let snapshot = self.0.get_snapshot();

        if name == b"ProcessNumberOfEntries" {
            args.property.set(&(snapshot.len() as u32));
            return Ok(());
        }

        let rust_idx = *args
            .table_idx
            .first()
            .ok_or(RBusError::ElementDoesNotExists)? as usize;
        let process = snapshot
            .get(rust_idx)
            .ok_or(RBusError::ElementDoesNotExists)?;

        match name {
            b"PID" => args.property.set(&(process.key().pid.0 as u32)),
            b"Command" => args.property.set(process.command()),
            b"Size" => {
                let kb = process
                    .last_memory_record
                    .as_ref()
                    .and_then(|(_, m)| m.resident_bytes)
                    .unwrap_or_default()
                    / 1024;
                args.property.set(&(kb as u32));
            }
            b"CPUTime" => {
                let ms = process
                    .last_cpu_record
                    .as_ref()
                    .and_then(|(_, c)| {
                        let ticks = c.counters.user_ticks.unwrap_or_default()
                            + c.counters.system_ticks.unwrap_or_default();
                        ticks
                            .saturating_mul(1000)
                            .checked_div(c.clock_ticks_per_second)
                    })
                    .unwrap_or_default();
                args.property.set(&(ms as u32));
            }
            b"X_RDK_Timestamp" => {
                let ts_ns = process
                    .last_cpu_record
                    .as_ref()
                    .map(|(ts, _)| ts.as_nanos() as u64)
                    .unwrap_or_default();
                args.property.set(&ts_ns);
            }
            _ => return Err(RBusError::ElementDoesNotExists),
        }
        Ok(())
    }
}
