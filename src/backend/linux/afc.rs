//! AFC file access through libimobiledevice.

use std::ffi::{CStr, CString, c_char};
use std::ptr;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::backend::Afc;

use super::ffi::{AFC_FOPEN_RDONLY, AFC_FOPEN_WRONLY, Libraries, afc_client_t};

/// Largest single `afc_file_read` call, in bytes.
const READ_CHUNK: u32 = 256 * 1024;

pub struct LinuxAfc {
    libraries: Arc<Libraries>,
    client: afc_client_t,
}

impl Drop for LinuxAfc {
    fn drop(&mut self) {
        if !self.client.is_null() {
            unsafe {
                (self.libraries.afc_client_free)(self.client);
            }
        }
    }
}

impl LinuxAfc {
    pub fn new(libraries: Arc<Libraries>, client: afc_client_t) -> Self {
        Self { libraries, client }
    }

    /// Reads a key/value string array from `afc_read_directory` or
    /// `afc_get_file_info`, then frees it.
    ///
    /// Both calls return a flat `[key, value, ..., NULL]` array whose memory is
    /// released by a single `afc_dictionary_free`, so the strings are copied out
    /// before the array is freed.
    fn read_string_array(&self, raw: *mut *mut c_char) -> Vec<String> {
        let mut values = Vec::new();
        if !raw.is_null() {
            let mut index = 0isize;
            loop {
                let entry = unsafe { *raw.offset(index) };
                if entry.is_null() {
                    break;
                }
                values.push(unsafe { CStr::from_ptr(entry) }.to_string_lossy().into_owned());
                index += 1;
            }
        }

        if !raw.is_null() {
            unsafe {
                (self.libraries.afc_dictionary_free)(raw);
            }
        }
        values
    }

    fn file_info(&self, path: &str) -> Option<Vec<String>> {
        let path = CString::new(path).ok()?;
        let mut raw: *mut *mut c_char = ptr::null_mut();
        let status = unsafe { (self.libraries.afc_get_file_info)(self.client, path.as_ptr(), &mut raw) };
        if status != 0 {
            return None;
        }
        Some(self.read_string_array(raw))
    }
}

impl Afc for LinuxAfc {
    fn exists(&self, path: &str) -> bool {
        self.file_info(path).is_some()
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let path_c = CString::new(path).context("AFC path contains a null byte")?;
        let mut handle: u64 = 0;
        let status = unsafe {
            (self.libraries.afc_file_open)(self.client, path_c.as_ptr(), AFC_FOPEN_RDONLY, &mut handle)
        };
        if status != 0 {
            bail!("afc_file_open('{}') failed with code {}", path, status);
        }

        let mut data = Vec::new();
        let mut buffer = vec![0u8; READ_CHUNK as usize];
        let result = loop {
            let mut received: u32 = 0;
            let status = unsafe {
                (self.libraries.afc_file_read)(
                    self.client,
                    handle,
                    buffer.as_mut_ptr() as *mut c_char,
                    READ_CHUNK,
                    &mut received,
                )
            };
            if status != 0 {
                break Err(anyhow::anyhow!(
                    "afc_file_read('{}') failed with code {}",
                    path,
                    status
                ));
            }
            if received == 0 {
                break Ok(());
            }
            data.extend_from_slice(&buffer[..received as usize]);
        };

        // A deferred flush error surfaces at close, so it must be checked
        // rather than discarded (the Windows backend does the same).
        let close_status = unsafe { (self.libraries.afc_file_close)(self.client, handle) };
        result?;
        if close_status != 0 {
            bail!("afc_file_close('{}') failed with code {}", path, close_status);
        }
        Ok(data)
    }

    fn write_file(&self, path: &str, data: &[u8]) -> Result<()> {
        let path_c = CString::new(path).context("AFC path contains a null byte")?;
        let mut handle: u64 = 0;
        let status = unsafe {
            (self.libraries.afc_file_open)(self.client, path_c.as_ptr(), AFC_FOPEN_WRONLY, &mut handle)
        };
        if status != 0 {
            bail!("afc_file_open('{}') for writing failed with code {}", path, status);
        }

        let mut written = 0usize;
        let result = loop {
            if written >= data.len() {
                break Ok(());
            }
            let mut chunk: u32 = 0;
            let status = unsafe {
                (self.libraries.afc_file_write)(
                    self.client,
                    handle,
                    data[written..].as_ptr() as *const c_char,
                    (data.len() - written) as u32,
                    &mut chunk,
                )
            };
            if status != 0 {
                break Err(anyhow::anyhow!(
                    "afc_file_write('{}') failed after {} bytes with code {}",
                    path,
                    written,
                    status
                ));
            }
            if chunk == 0 {
                break Err(anyhow::anyhow!(
                    "afc_file_write('{}') stopped after {} of {} bytes",
                    path,
                    written,
                    data.len()
                ));
            }
            written += chunk as usize;
        };

        // A deferred flush error surfaces at close, so it must be checked
        // rather than discarded (the Windows backend does the same).
        let close_status = unsafe { (self.libraries.afc_file_close)(self.client, handle) };
        result?;
        if close_status != 0 {
            bail!("afc_file_close('{}') failed with code {}", path, close_status);
        }
        Ok(())
    }

    fn make_directory(&self, path: &str) -> Result<()> {
        let path_c = CString::new(path).context("AFC path contains a null byte")?;
        let status = unsafe { (self.libraries.afc_make_directory)(self.client, path_c.as_ptr()) };
        // Creating a directory that already exists is not an error for callers.
        if status != 0 && !self.exists(path) {
            bail!("afc_make_directory('{}') failed with code {}", path, status);
        }
        Ok(())
    }

    fn remove_path(&self, path: &str) -> Result<()> {
        if !self.exists(path) {
            return Ok(());
        }
        let path_c = CString::new(path).context("AFC path contains a null byte")?;
        let status = unsafe { (self.libraries.afc_remove_path)(self.client, path_c.as_ptr()) };
        // The device may have removed the path between the check and the call.
        if status != 0 && self.exists(path) {
            bail!("afc_remove_path('{}') failed with code {}", path, status);
        }
        Ok(())
    }

    fn list_directory(&self, path: &str) -> Result<Vec<String>> {
        let path_c = CString::new(path).context("AFC path contains a null byte")?;
        let mut raw: *mut *mut c_char = ptr::null_mut();
        let status = unsafe { (self.libraries.afc_read_directory)(self.client, path_c.as_ptr(), &mut raw) };
        if status != 0 {
            bail!("afc_read_directory('{}') failed with code {}", path, status);
        }

        let mut names = self.read_string_array(raw);
        names.retain(|name| name != "." && name != "..");
        Ok(names)
    }
}
