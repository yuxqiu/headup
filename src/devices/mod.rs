//! Earbud backends implementing [`HeadTracker`], how to recognise them, and [`AnyTracker`] so the
//! rest of the program never names a model. Adding a model: a backend module, a [`Model`] variant
//! with its name rule, and an [`AnyTracker`] variant with its arms below.

pub mod freeclip2;

use crate::core::tracker::{Event, HeadTracker};
use anyhow::Result;
use bluer::Address;
use std::time::Duration;

/// Earbud models that have a backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Model {
    FreeClip2,
}

impl Model {
    pub const ALL: [Model; 1] = [Model::FreeClip2];

    /// Recognise a model from its Bluetooth name.
    pub fn from_bluetooth_name(name: &str) -> Option<Model> {
        Model::ALL.into_iter().find(|m| m.matches(name))
    }

    fn matches(self, name: &str) -> bool {
        match self {
            // Same rule as the vendor app's device whitelist: `^(?=(HUAWEI FreeClip 2))`.
            Model::FreeClip2 => name.starts_with("HUAWEI FreeClip 2"),
        }
    }
}

/// A connected Bluetooth device that a backend supports.
#[derive(Debug, Clone)]
pub struct Found {
    pub address: Address,
    pub name: String,
    pub model: Model,
}

/// Connected devices on the default BlueZ adapter that have a backend.
pub async fn connected_supported(session: &bluer::Session) -> Result<Vec<Found>> {
    let adapter = session.default_adapter().await?;
    let mut found = Vec::new();
    for address in adapter.device_addresses().await? {
        let device = adapter.device(address)?;
        if !device.is_connected().await? {
            continue;
        }
        let Some(name) = device.name().await? else {
            continue;
        };
        if let Some(model) = Model::from_bluetooth_name(&name) {
            found.push(Found {
                address,
                name,
                model,
            });
        }
    }
    Ok(found)
}

/// A connected backend of any supported model.
pub enum AnyTracker {
    FreeClip2(freeclip2::FreeClip2),
}

impl AnyTracker {
    /// Connect to `found` and start head tracking. `raw_log` receives backend-specific raw samples.
    pub async fn connect(found: &Found, raw_log: Option<std::fs::File>) -> Result<Self> {
        Ok(match found.model {
            Model::FreeClip2 => {
                AnyTracker::FreeClip2(freeclip2::FreeClip2::connect(found.address, raw_log).await?)
            }
        })
    }
}

impl HeadTracker for AnyTracker {
    async fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
        match self {
            AnyTracker::FreeClip2(t) => t.next_event(timeout).await,
        }
    }

    async fn stop(&mut self) -> Result<()> {
        match self {
            AnyTracker::FreeClip2(t) => t.stop().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_models_by_name() {
        assert_eq!(
            Model::from_bluetooth_name("HUAWEI FreeClip 2"),
            Some(Model::FreeClip2)
        );
        assert_eq!(
            Model::from_bluetooth_name("HUAWEI FreeClip 2 典藏版"),
            Some(Model::FreeClip2)
        );
        assert_eq!(Model::from_bluetooth_name("HUAWEI FreeClip"), None); // 1st gen: no backend
        assert_eq!(Model::from_bluetooth_name("Magic Trackpad"), None);
    }
}
