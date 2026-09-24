//! Windows platform services that are unrelated to Apple's libraries.

use std::ffi::c_void;
use std::time::Duration;

#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(
        h_algorithm: *mut c_void,
        buffer: *mut u8,
        buffer_len: u32,
        flags: u32,
    ) -> i32;
}

unsafe extern "system" {
    fn setsockopt(socket: usize, level: i32, option: i32, value: *const i8, value_len: i32)
    -> i32;
}

const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 2;
const SOL_SOCKET: i32 = 0xffff;
const SO_RCVTIMEO: i32 = 0x1006;

pub fn random_bytes(buffer: &mut [u8]) {
    unsafe {
        let _ = BCryptGenRandom(
            std::ptr::null_mut(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        );
    }
}

/// Limits how long a receive on `socket` may block.
pub fn set_receive_timeout(socket: usize, timeout: Duration) {
    let timeout_ms = timeout.as_millis().min(u32::MAX as u128) as u32;
    unsafe {
        let _ = setsockopt(
            socket,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &timeout_ms as *const u32 as *const i8,
            std::mem::size_of::<u32>() as i32,
        );
    }
}
