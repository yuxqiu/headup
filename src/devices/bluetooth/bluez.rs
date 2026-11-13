//! Bluetooth on Linux, through BlueZ's D-Bus API (`bluer`).

use super::{Address, Device};
use anyhow::{Context, Result};

/// A connection to an RFCOMM channel.
pub type RfcommStream = bluer::rfcomm::Stream;

/// The computer's Bluetooth stack: BlueZ, through its default adapter.
pub struct Host {
    session: bluer::Session,
}

impl Host {
    pub async fn new() -> Result<Host> {
        let session = bluer::Session::new()
            .await
            .context("connect to BlueZ over D-Bus")?;
        Ok(Host { session })
    }

    async fn adapter(&self) -> Result<bluer::Adapter> {
        self.session
            .default_adapter()
            .await
            .context("default adapter")
    }

    /// This computer's adapter address.
    pub async fn adapter_address(&self) -> Result<Address> {
        let address = self.adapter().await?.address().await;
        Ok(Address(address.context("adapter address")?.0))
    }

    /// The devices the computer is connected to that have a name.
    pub async fn connected(&self) -> Result<Vec<Device>> {
        let adapter = self.adapter().await?;
        let mut connected = Vec::new();
        for address in adapter.device_addresses().await.context("device list")? {
            let device = adapter.device(address).context("device")?;
            // BlueZ forgets nearby BLE devices at any time. A device can vanish between listing
            // and querying it ("target object was either not present or removed"), so skip it.
            let (Ok(is_connected), Ok(name)) = (device.is_connected().await, device.name().await)
            else {
                continue;
            };
            if let Some(name) = name.filter(|_| is_connected) {
                connected.push(Device {
                    address: Address(address.0),
                    name,
                });
            }
        }
        Ok(connected)
    }

    /// The name BlueZ knows `address` by. The device need not be connected. `None` when BlueZ has
    /// no name for it or cannot be asked (a device BlueZ has not seen yet is not an error).
    pub async fn name_of(&self, address: Address) -> Result<Option<String>> {
        let device = self.adapter().await?.device(bluer::Address(address.0))?;
        Ok(device.name().await.ok().flatten())
    }

    /// Connect to RFCOMM `channel` on the device at `address`.
    pub async fn connect_rfcomm(&self, address: Address, channel: u8) -> Result<RfcommStream> {
        let target = bluer::rfcomm::SocketAddr::new(bluer::Address(address.0), channel);
        Ok(RfcommStream::connect(target).await?)
    }
}
