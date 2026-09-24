//! Linux backend: libimobiledevice over usbmuxd.
//!
//! Apple's private MobileDevice / AirTrafficHost libraries exist only on
//! Windows, so this backend reaches the same device services through the public
//! libimobiledevice stack: `libimobiledevice` for lockdown, AFC and raw service
//! sockets, `libusbmuxd` for the pair record, and `libplist` for property lists.

mod afc;
mod at;
mod ffi;
mod pairing;
mod plist_bridge;
mod session;
mod usbmux;

pub use at::MessageHandle;
pub use ffi::verify_support;
pub use session::{open_air_traffic, open_session, read_device_identity};
pub use usbmux::list_usbmux_devices;

use anyhow::{Context, Result};

/// Fills the buffer with bytes from the kernel random source.
pub fn random_bytes(buffer: &mut [u8]) -> Result<()> {
    use std::io::Read;

    let mut source = std::fs::File::open("/dev/urandom").context("Failed to open /dev/urandom")?;
    source
        .read_exact(buffer)
        .context("Failed to read from /dev/urandom")
}
