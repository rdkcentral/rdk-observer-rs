//! RBus provider exposing TR-181 `Device.DeviceInfo.ProcessStatus.` data model.

use anyhow::Context;
use rbus_core::RBusError;
use rbus_provider::element::RBusProviderElement;
use rbus_provider::element::object::rbus_object;
use rbus_provider::element::property::{RBusProviderGetter, RBusProviderGetterArgs, rbus_property};
use rbus_provider::element::table::{RBusProviderTableSync, RBusProviderTableSyncArgs, rbus_table};
use rbus_provider::provider::{RBusProvider, RBusProviderError};
use rdk_observer_core::process_registry::ProcessRegistry;
use std::ffi::CStr;
use tracing::{error, info, instrument};

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
    #[instrument(name = "rbus", skip_all)]
    pub fn open(registry: ProcessRegistry) -> Option<Self> {
        match RBusProvider::open(COMPONENT_NAME, move || Self::build_model(registry)) {
            Ok(e) => {
                info!("RBus elements successfully registered");
                Some(Self { _handle: e })
            }
            Err(e) => {
                error!("RBus elements registration failed: {e}");
                None
            }
        }
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
#[allow(non_camel_case_types)]
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

        let Some(process_idx) = args.table_idx.first() else {
            return Err(RBusError::ElementDoesNotExists);
        };

        let Some(process) = snapshot.get(*process_idx as usize) else {
            return Err(RBusError::ElementDoesNotExists);
        };

        match name {
            b"PID" => args.property.set(&(process.key().pid.0 as u32)),
            b"Command" => args.property.set(process.command()),
            b"Size" => {
                let record = process.last_memory_record.as_ref();
                let bytes = record.and_then(|(_, e)| e.resident_bytes).unwrap_or(0);
                args.property.set(&((bytes as u32) / 1024));
            }
            b"CPUTime" => {
                let record = process.last_cpu_record.as_ref();
                let cpu_ms = record.and_then(|(_, e)| {
                    let ticks = e.counters.user_ticks? + e.counters.system_ticks?;
                    Some(ticks.saturating_mul(1000) / e.clock_ticks_per_second)
                });
                args.property.set(&(cpu_ms.unwrap_or(0) as u32));
            }
            b"X_RDK_Timestamp" => {
                let record = process.last_cpu_record.as_ref();
                let ts_ns = record.map(|(ts, _)| ts.as_nanos() as u64);
                args.property.set(&ts_ns.unwrap_or(0));
            }
            _ => return Err(RBusError::ElementDoesNotExists),
        }
        Ok(())
    }
}
