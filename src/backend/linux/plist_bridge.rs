//! Conversion between the `plist` crate values and libplist node trees.
//!
//! Decoding goes through the binary property list representation, so this
//! module needs only `plist_to_bin` and `plist_free` instead of a full type
//! walk over the node tree.

use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use super::ffi::{Libraries, plist_t};

/// Owns one libplist node tree and frees it on drop.
pub struct PlistHandle {
    libraries: Arc<Libraries>,
    raw: plist_t,
}

impl Drop for PlistHandle {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe {
                (self.libraries.plist_free)(self.raw);
            }
        }
    }
}

impl PlistHandle {
    /// Takes ownership of a node tree that libimobiledevice handed out.
    pub fn from_raw(libraries: Arc<Libraries>, raw: plist_t) -> Self {
        Self { libraries, raw }
    }

    /// Decodes one binary property list node tree into a `plist` crate value.
    pub fn to_value(&self) -> Result<plist::Value> {
        let mut bytes: *mut std::ffi::c_char = ptr::null_mut();
        let mut length: u32 = 0;
        let status = unsafe { (self.libraries.plist_to_bin)(self.raw, &mut bytes, &mut length) };
        if status != 0 || bytes.is_null() {
            bail!("plist_to_bin failed with code {}", status);
        }

        let slice = unsafe { std::slice::from_raw_parts(bytes as *const u8, length as usize) };
        let owned = slice.to_vec();
        unsafe {
            (self.libraries.plist_mem_free)(bytes as *mut c_void);
        }

        plist::Value::from_reader(std::io::Cursor::new(owned))
            .context("Failed to decode a binary property list")
    }
}
