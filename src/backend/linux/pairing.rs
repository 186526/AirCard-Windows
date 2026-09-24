//! HostID lookup from the existing usbmuxd pair record.
//!
//! libimobiledevice's `lockdownd_client_new_with_handshake` calls
//! `lockdownd_pair` when no pair record exists (`src/lockdown.c`), which AirCard
//! never wants: pairing is the user's decision, made once over USB in iTunes or
//! Apple Devices. Reading the stored record and passing its `HostID` to
//! `lockdownd_start_session` reaches the same session without that side effect.

use std::ffi::{CString, c_char};
use std::ptr;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use super::ffi::Libraries;

/// Largest pair record accepted from usbmuxd, in bytes.
const MAX_PAIR_RECORD: u32 = 1024 * 1024;

/// Returns the `HostID` of an already paired device.
pub fn host_id(libraries: &Arc<Libraries>, udid: &str) -> Result<String> {
    let udid_c = CString::new(udid).context("Device UDID contains a null byte")?;
    let mut record: *mut c_char = ptr::null_mut();
    let mut size: u32 = 0;
    let status =
        unsafe { (libraries.usbmuxd_read_pair_record)(udid_c.as_ptr(), &mut record, &mut size) };

    if status != 0 || record.is_null() || size == 0 {
        if !record.is_null() {
            unsafe { libc::free(record as *mut std::ffi::c_void) };
        }
        bail!(
            "No pairing record for {} (usbmuxd reported {}). Pair the iPhone once over USB and tap Trust.",
            udid,
            status
        );
    }
    if size > MAX_PAIR_RECORD {
        unsafe { libc::free(record as *mut std::ffi::c_void) };
        bail!("The pairing record for {} is implausibly large", udid);
    }

    let bytes = unsafe { std::slice::from_raw_parts(record as *const u8, size as usize) }.to_vec();
    // `usbmuxd_read_pair_record` allocates with `malloc`, so `free` is the
    // matching deallocator; `plist_mem_free` would be wrong here.
    unsafe { libc::free(record as *mut std::ffi::c_void) };

    decode_record(&bytes)?
        .as_dictionary()
        .and_then(|dict| dict.get("HostID"))
        .and_then(|value| value.as_string())
        .map(str::to_string)
        .context("The pairing record carries no HostID")
}

/// usbmuxd hands the record back as either an XML or a binary property list.
fn decode_record(record: &[u8]) -> Result<plist::Value> {
    plist::Value::from_reader(std::io::Cursor::new(record))
        .context("Failed to decode the pairing record")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair_record(host_id: &str) -> plist::Dictionary {
        let mut record = plist::Dictionary::new();
        record.insert(
            "HostID".to_string(),
            plist::Value::String(host_id.to_string()),
        );
        record.insert(
            "SystemBUID".to_string(),
            plist::Value::String("00000000-0000-0000-0000-000000000000".to_string()),
        );
        record
    }

    /// usbmuxd returns the record in whichever format it was stored in, so both
    /// must decode to the same `HostID`.
    #[test]
    fn test_decode_record_handles_both_plist_formats() {
        let expected = "A1B2C3D4-0000-1111-2222-333344445555";

        let mut binary = Vec::new();
        plist::to_writer_binary(&mut binary, &plist::Value::Dictionary(pair_record(expected)))
            .unwrap();
        assert!(binary.starts_with(b"bplist00"));

        let mut xml = Vec::new();
        plist::to_writer_xml(&mut xml, &plist::Value::Dictionary(pair_record(expected))).unwrap();

        for encoded in [binary, xml] {
            let host_id = decode_record(&encoded)
                .unwrap()
                .as_dictionary()
                .and_then(|dict| dict.get("HostID"))
                .and_then(|value| value.as_string())
                .map(str::to_string);

            assert_eq!(host_id.as_deref(), Some(expected));
        }
    }

    /// A truncated or garbage record must be an error, never a silent success.
    #[test]
    fn test_decode_record_rejects_garbage() {
        assert!(decode_record(b"not a property list").is_err());
        assert!(decode_record(&[]).is_err());
    }
}
