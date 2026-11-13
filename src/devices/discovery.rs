//! Finding connected earbuds that have a backend.

use super::Model;
use super::bluetooth::{Address, Device, Host};
use anyhow::{Result, bail};

/// A connected Bluetooth device that a backend supports.
#[derive(Debug, Clone)]
pub struct Found {
    pub device: Device,
    pub model: Model,
    /// This computer's Bluetooth adapter, as the earbuds know it.
    pub adapter: Address,
}

/// The connected supported earbuds at `want`, or the first connected supported earbuds if `want`
/// is `None`. `None` if there are none.
pub async fn find(host: &Host, want: Option<Address>) -> Result<Option<Found>> {
    let mut found = connected(host).await?.into_iter();
    Ok(match want {
        Some(address) => found.find(|f| f.device.address == address),
        None => found.next(),
    })
}

/// Fail if the computer knows `address` by a name that no backend recognises.
pub async fn ensure_supported(host: &Host, address: Address) -> Result<()> {
    // An unknown name passes: the computer may not have seen the device yet, and the run loop
    // then waits for it to connect as a supported model.
    if let Some(name) = host.name_of(address).await?
        && Model::from_bluetooth_name(&name).is_none()
    {
        bail!("{address} ({name}) is not a supported device (see README)");
    }
    Ok(())
}

/// Connected devices that have a backend.
async fn connected(host: &Host) -> Result<Vec<Found>> {
    let adapter = host.adapter_address().await?;
    Ok(host
        .connected()
        .await?
        .into_iter()
        .filter_map(|device| {
            let model = Model::from_bluetooth_name(&device.name)?;
            Some(Found {
                device,
                model,
                adapter,
            })
        })
        .collect())
}
