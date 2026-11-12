//! Finding connected earbuds that have a backend, through BlueZ.

use super::Model;
use anyhow::{Context, Result, bail};
use bluer::Address;

/// A connected Bluetooth device that a backend supports.
#[derive(Debug, Clone)]
pub struct Found {
    pub address: Address,
    pub name: String,
    pub model: Model,
    /// This computer's Bluetooth adapter, as the earbuds know it.
    pub adapter: Address,
}

/// The connected supported earbuds at `want`, or the first connected supported earbuds if `want`
/// is `None`; `None` if there are none.
pub async fn find(session: &bluer::Session, want: Option<Address>) -> Result<Option<Found>> {
    let mut found = connected(session).await?.into_iter();
    Ok(match want {
        Some(address) => found.find(|f| f.address == address),
        None => found.next(),
    })
}

/// Fail if BlueZ knows `address` by a name that no backend recognises.
pub async fn ensure_supported(session: &bluer::Session, address: Address) -> Result<()> {
    let device = session.default_adapter().await?.device(address)?;
    if let Ok(Some(name)) = device.name().await
        && Model::from_bluetooth_name(&name).is_none()
    {
        bail!("{address} ({name}) is not a supported device (see README)");
    }
    Ok(())
}

/// Connected devices on the default BlueZ adapter that have a backend.
async fn connected(session: &bluer::Session) -> Result<Vec<Found>> {
    let adapter = session.default_adapter().await.context("default adapter")?;
    let adapter_address = adapter.address().await.context("adapter address")?;
    let mut found = Vec::new();
    for address in adapter.device_addresses().await.context("device list")? {
        let device = adapter.device(address).context("device")?;
        // BlueZ forgets nearby BLE devices at any time, so a device can vanish between listing and
        // querying it ("target object was either not present or removed"); skip it.
        let (Ok(connected), Ok(name)) = (device.is_connected().await, device.name().await) else {
            continue;
        };
        let Some(name) = name.filter(|_| connected) else {
            continue;
        };
        if let Some(model) = Model::from_bluetooth_name(&name) {
            found.push(Found {
                address,
                name,
                model,
                adapter: adapter_address,
            });
        }
    }
    Ok(found)
}
