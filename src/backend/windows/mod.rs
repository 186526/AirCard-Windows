//! Windows backend: Apple's MobileDevice and AirTrafficHost libraries.

mod afc;
mod at;
mod ffi;
mod misc;
mod session;
mod usbmux;

pub use at::{MessageHandle, open_air_traffic};
pub use ffi::verify_support;
pub use misc::random_bytes;
pub use session::{open_session, read_device_identity};
pub use usbmux::list_usbmux_devices;
