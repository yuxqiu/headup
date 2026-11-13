//! Earbud backends, how to recognise them, and [`AnyTracker`] so the rest of the program never
//! names a model.
//!
//! Adding a model: write a backend module whose tracker implements [`HeadTracker`] and
//! [`Backend`], declare the module and add one line to the `backends!` list below.

pub mod bluetooth;
mod discovery;

pub use discovery::{Found, ensure_supported, find};

use crate::core::record::DeviceLog;
use crate::core::tracker::{Event, HeadTracker, TrackerTraits};
use anyhow::Result;
use bluetooth::Host;
use std::time::Duration;

/// What the registry needs from a backend on top of [`HeadTracker`].
#[allow(async_fn_in_trait)] // used through generics only, not as dyn or across threads
pub trait Backend: HeadTracker + Sized {
    /// Whether a Bluetooth device name belongs to this model.
    fn recognises(bluetooth_name: &str) -> bool;

    /// Connect to `found` through `host` and start head tracking. `log` is for raw device data
    /// that helps debug the backend (`--raw-log`).
    async fn connect(host: &Host, found: &Found, log: DeviceLog) -> Result<Self>;
}

/// Generates [`Model`] (one variant per backend) and [`AnyTracker`] (dispatch to the connected
/// backend) from the list of `Variant => backend type` pairs. When several backends recognise a
/// name, the first listed wins.
macro_rules! backends {
    ($($model:ident => $backend:ty),+ $(,)?) => {
        /// Earbud models that have a backend.
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub enum Model {
            $($model),+
        }

        impl Model {
            /// Recognise a model from its Bluetooth name.
            pub fn from_bluetooth_name(name: &str) -> Option<Model> {
                $(if <$backend as Backend>::recognises(name) {
                    return Some(Model::$model);
                })+
                None
            }
        }

        /// A connected backend of any supported model.
        pub enum AnyTracker {
            $($model($backend)),+
        }

        impl AnyTracker {
            /// Connect to `found` with its model's backend; see [`Backend::connect`].
            pub async fn connect(host: &Host, found: &Found, log: DeviceLog) -> Result<Self> {
                Ok(match found.model {
                    $(Model::$model => AnyTracker::$model(
                        <$backend as Backend>::connect(host, found, log).await?,
                    ),)+
                })
            }
        }

        impl HeadTracker for AnyTracker {
            async fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
                match self {
                    $(AnyTracker::$model(t) => t.next_event(timeout).await,)+
                }
            }

            async fn stop(&mut self) -> Result<()> {
                match self {
                    $(AnyTracker::$model(t) => t.stop().await,)+
                }
            }

            fn traits(&self) -> TrackerTraits {
                match self {
                    $(AnyTracker::$model(t) => t.traits(),)+
                }
            }
        }
    };
}

pub mod freeclip2;

backends! {
    FreeClip2 => freeclip2::FreeClip2,
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
