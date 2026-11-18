use kinewright_job_sys::ProcessHandle;
use std::{os::fd::AsFd, time::Duration};
use zbus::{
    blocking::{Connection, Proxy, connection::Builder},
    zvariant::{Fd, OwnedObjectPath, Value},
};

const DESTINATION: &str = "org.freedesktop.systemd1";
const ROOT: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const SCOPE: &str = "org.freedesktop.systemd1.Scope";

pub struct Bus(Connection);
impl Bus {
    pub fn connect() -> zbus::Result<Self> {
        // The launcher supplies this sole environment value. No HOME/XDG or
        // autolaunch fallback can turn absence into another connection route.
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .map_err(|_| zbus::Error::Failure("session manager unavailable".into()))?;
        Self::connect_address(&address)
    }

    pub(super) fn connect_address(address: &str) -> zbus::Result<Self> {
        let connection = Builder::address(address)?
            .method_timeout(Duration::from_secs(2))
            .build()?;
        let daemon = Proxy::new(
            &connection,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )?;
        let present: bool = daemon.call("NameHasOwner", &DESTINATION)?;
        if !present {
            return Err(zbus::Error::Failure("user manager unavailable".into()));
        }
        drop(daemon);
        Ok(Self(connection))
    }
    #[cfg(feature = "slow-tests")]
    pub fn disconnect(self) -> zbus::Result<()> {
        self.0.close()
    }

    fn manager(&self) -> zbus::Result<Proxy<'_>> {
        Proxy::new(&self.0, DESTINATION, ROOT, MANAGER)
    }
    pub fn start(&self, name: &str, process: &ProcessHandle, memory: u64) -> zbus::Result<()> {
        // PIDFDs binds systemd's delayed operation to the pinned task. A plain
        // numeric PID could refer to somebody else after app/supervisor loss.
        // Supported by systemd v255 (Ubuntu 24.04) and the qualified Arch v262.
        let properties: Vec<(&str, Value<'_>)> = vec![
            (
                "Description",
                Value::from("Kinewright confined media probe"),
            ),
            ("PIDFDs", Value::from(vec![Fd::from(process.as_fd())])),
            ("MemoryMax", Value::from(memory)),
            ("MemorySwapMax", Value::from(0_u64)),
            ("TasksMax", Value::from(8_u64)),
            ("CPUQuotaPerSecUSec", Value::from(1_000_000_u64)),
            ("OOMPolicy", Value::from("kill")),
            ("MemoryAccounting", Value::from(true)),
            ("CPUAccounting", Value::from(true)),
            ("TasksAccounting", Value::from(true)),
            ("CollectMode", Value::from("inactive-or-failed")),
            ("TimeoutStopUSec", Value::from(250_000_u64)),
        ];
        let auxiliary: Vec<(&str, Vec<(&str, Value<'_>)>)> = vec![];
        let _: OwnedObjectPath = self
            .manager()?
            .call("StartTransientUnit", &(name, "fail", properties, auxiliary))?;
        Ok(())
    }
    pub fn unit(&self, name: &str) -> zbus::Result<OwnedObjectPath> {
        self.manager()?.call("GetUnit", &name)
    }
    pub fn group(&self, path: &OwnedObjectPath) -> zbus::Result<String> {
        Proxy::new(&self.0, DESTINATION, path.as_str(), SCOPE)?.get_property("ControlGroup")
    }
    pub fn active(&self, path: &OwnedObjectPath) -> zbus::Result<bool> {
        let state: String = Proxy::new(
            &self.0,
            DESTINATION,
            path.as_str(),
            "org.freedesktop.systemd1.Unit",
        )?
        .get_property("ActiveState")?;
        Ok(state == "active")
    }
    pub fn verify_controls(&self, path: &OwnedObjectPath, memory: u64) -> zbus::Result<bool> {
        let proxy = Proxy::new(&self.0, DESTINATION, path.as_str(), SCOPE)?;
        for (property, expected) in [
            ("MemoryMax", memory),
            ("MemorySwapMax", 0),
            ("TasksMax", 8),
            ("CPUQuotaPerSecUSec", 1_000_000),
        ] {
            if proxy.get_property::<u64>(property)? != expected {
                return Ok(false);
            }
        }
        Ok(proxy.get_property::<String>("OOMPolicy")? == "kill")
    }
    pub fn stop(&self, name: &str) -> zbus::Result<()> {
        let result: zbus::Result<OwnedObjectPath> =
            self.manager()?.call("StopUnit", &(name, "replace"));
        match result {
            Ok(_) => Ok(()),
            Err(error) if absent(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }
    pub fn reset(&self, name: &str) -> zbus::Result<()> {
        let result: zbus::Result<()> = self.manager()?.call("ResetFailedUnit", &name);
        match result {
            Ok(()) => Ok(()),
            Err(error) if absent(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }
}
pub fn absent(error: &zbus::Error) -> bool {
    matches!(error,zbus::Error::MethodError(name,_,_) if name.as_str()=="org.freedesktop.systemd1.NoSuchUnit")
}
