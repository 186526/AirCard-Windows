//! Runtime loading of libimobiledevice and libplist.
//!
//! The libraries are opened by soname so that no `-dev` package (headers or the
//! unversioned `.so` symlink) is needed to build or run AirCard. The signatures
//! below match libimobiledevice 1.4.0 (`include/libimobiledevice/*.h`) and
//! libplist 2.7.0 (`include/plist/plist.h`).

#![allow(non_snake_case, non_camel_case_types)]

use std::ffi::{c_char, c_int, c_uint, c_void};
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, bail};
use libloading::{Library, Symbol};

pub type idevice_t = *mut c_void;
pub type idevice_connection_t = *mut c_void;
pub type lockdownd_client_t = *mut c_void;
pub type afc_client_t = *mut c_void;
pub type plist_t = *mut c_void;

/// `struct lockdownd_service_descriptor` from `lockdown.h`.
#[repr(C)]
pub struct lockdownd_service_descriptor {
    pub port: u16,
    pub ssl_enabled: u8,
    pub identifier: *mut c_char,
}

pub type lockdownd_service_descriptor_t = *mut lockdownd_service_descriptor;

/// `struct idevice_info` from `libimobiledevice.h`.
#[repr(C)]
pub struct idevice_info {
    pub udid: *mut c_char,
    pub conn_type: c_int,
    pub conn_data: *mut c_void,
}

pub type idevice_info_t = *mut idevice_info;

pub const IDEVICE_LOOKUP_USBMUX: c_int = 1 << 1;
pub const IDEVICE_LOOKUP_NETWORK: c_int = 1 << 2;

pub const CONNECTION_USBMUXD: c_int = 1;
pub const CONNECTION_NETWORK: c_int = 2;

/// `AFC_FOPEN_RDONLY` from `afc.h`.
pub const AFC_FOPEN_RDONLY: c_int = 0x0000_0001;
/// `AFC_FOPEN_WRONLY` from `afc.h`.
pub const AFC_FOPEN_WRONLY: c_int = 0x0000_0003;

/// `IDEVICE_E_NOT_ENOUGH_DATA` from `libimobiledevice.h`. Returned when a
/// receive would block, so it means the same thing as a timeout here.
pub const IDEVICE_E_NOT_ENOUGH_DATA: c_int = -4;
/// `IDEVICE_E_TIMEOUT` from `libimobiledevice.h`.
pub const IDEVICE_E_TIMEOUT: c_int = -7;

const DEVICE_LIBRARY_NAMES: &[&str] = &["libimobiledevice-1.0.so.6", "libimobiledevice-1.0.so"];
const PLIST_LIBRARY_NAMES: &[&str] = &["libplist-2.0.so.4", "libplist-2.0.so"];
const USBMUXD_LIBRARY_NAMES: &[&str] = &["libusbmuxd-2.0.so.7", "libusbmuxd-2.0.so"];

/// Every symbol AirCard uses, resolved once for the whole process.
#[allow(dead_code)]
pub struct Libraries {
    _device_library: Library,
    _plist_library: Library,
    _usbmuxd_library: Library,
    /// Name of the libimobiledevice file that was loaded.
    pub source: String,

    pub idevice_get_device_list_extended:
        unsafe extern "C" fn(*mut *mut idevice_info_t, *mut c_int) -> c_int,
    pub idevice_device_list_extended_free: unsafe extern "C" fn(*mut idevice_info_t) -> c_int,
    pub idevice_new_with_options:
        unsafe extern "C" fn(*mut idevice_t, *const c_char, c_int) -> c_int,
    pub idevice_free: unsafe extern "C" fn(idevice_t) -> c_int,
    pub idevice_get_udid: unsafe extern "C" fn(idevice_t, *mut *mut c_char) -> c_int,

    pub idevice_connect: unsafe extern "C" fn(idevice_t, u16, *mut idevice_connection_t) -> c_int,
    pub idevice_disconnect: unsafe extern "C" fn(idevice_connection_t) -> c_int,
    pub idevice_connection_send:
        unsafe extern "C" fn(idevice_connection_t, *const c_char, u32, *mut u32) -> c_int,
    pub idevice_connection_receive_timeout: unsafe extern "C" fn(
        idevice_connection_t,
        *mut c_char,
        u32,
        *mut u32,
        c_uint,
    ) -> c_int,
    pub idevice_connection_enable_ssl: unsafe extern "C" fn(idevice_connection_t) -> c_int,

    pub lockdownd_client_new:
        unsafe extern "C" fn(idevice_t, *mut lockdownd_client_t, *const c_char) -> c_int,
    pub lockdownd_client_free: unsafe extern "C" fn(lockdownd_client_t) -> c_int,
    pub lockdownd_start_session: unsafe extern "C" fn(
        lockdownd_client_t,
        *const c_char,
        *mut *mut c_char,
        *mut c_int,
    ) -> c_int,
    pub lockdownd_get_value: unsafe extern "C" fn(
        lockdownd_client_t,
        *const c_char,
        *const c_char,
        *mut plist_t,
    ) -> c_int,
    pub lockdownd_start_service: unsafe extern "C" fn(
        lockdownd_client_t,
        *const c_char,
        *mut lockdownd_service_descriptor_t,
    ) -> c_int,
    pub lockdownd_service_descriptor_free:
        unsafe extern "C" fn(lockdownd_service_descriptor_t),

    pub afc_client_new:
        unsafe extern "C" fn(idevice_t, lockdownd_service_descriptor_t, *mut afc_client_t) -> c_int,
    pub afc_client_free: unsafe extern "C" fn(afc_client_t) -> c_int,
    pub afc_read_directory:
        unsafe extern "C" fn(afc_client_t, *const c_char, *mut *mut *mut c_char) -> c_int,
    pub afc_get_file_info:
        unsafe extern "C" fn(afc_client_t, *const c_char, *mut *mut *mut c_char) -> c_int,
    pub afc_dictionary_free: unsafe extern "C" fn(*mut *mut c_char),
    pub afc_file_open:
        unsafe extern "C" fn(afc_client_t, *const c_char, c_int, *mut u64) -> c_int,
    pub afc_file_close: unsafe extern "C" fn(afc_client_t, u64) -> c_int,
    pub afc_file_read:
        unsafe extern "C" fn(afc_client_t, u64, *mut c_char, u32, *mut u32) -> c_int,
    pub afc_file_write:
        unsafe extern "C" fn(afc_client_t, u64, *const c_char, u32, *mut u32) -> c_int,
    pub afc_make_directory: unsafe extern "C" fn(afc_client_t, *const c_char) -> c_int,
    pub afc_remove_path: unsafe extern "C" fn(afc_client_t, *const c_char) -> c_int,

    pub plist_to_bin: unsafe extern "C" fn(plist_t, *mut *mut c_char, *mut u32) -> c_int,
    pub plist_free: unsafe extern "C" fn(plist_t),
    pub plist_mem_free: unsafe extern "C" fn(*mut c_void),

    pub usbmuxd_read_pair_record:
        unsafe extern "C" fn(*const c_char, *mut *mut c_char, *mut u32) -> c_int,
}

static LIBRARIES: OnceLock<Arc<Libraries>> = OnceLock::new();

pub fn libraries() -> Result<Arc<Libraries>> {
    if let Some(libraries) = LIBRARIES.get() {
        return Ok(Arc::clone(libraries));
    }

    let libraries = Arc::new(load_libraries()?);
    let _ = LIBRARIES.set(Arc::clone(&libraries));
    Ok(libraries)
}

fn load_libraries() -> Result<Libraries> {
    let (device_library, source) = open_first(DEVICE_LIBRARY_NAMES)
        .context("libimobiledevice was not found. Install it (Debian/Ubuntu: libimobiledevice-1.0-6, Fedora: libimobiledevice) to talk to an iPhone.")?;
    let (plist_library, _) = open_first(PLIST_LIBRARY_NAMES)
        .context("libplist was not found. Install libimobiledevice and its libplist dependency.")?;
    let (usbmuxd_library, _) = open_first(USBMUXD_LIBRARY_NAMES)
        .context("libusbmuxd was not found. Install libimobiledevice and its libusbmuxd dependency.")?;

    // Resolving a symbol from a library AirCard just opened is sound: the
    // `Library` values below keep the images mapped for the process lifetime.
    macro_rules! load_sym {
        ($lib:expr, $name:expr) => {{
            let symbol: Symbol<_> = unsafe {
                $lib.get($name.as_bytes())
                    .context(concat!("Missing symbol: ", $name))?
            };
            *symbol
        }};
    }

    Ok(Libraries {
        idevice_get_device_list_extended: load_sym!(
            device_library,
            "idevice_get_device_list_extended"
        ),
        idevice_device_list_extended_free: load_sym!(
            device_library,
            "idevice_device_list_extended_free"
        ),
        idevice_new_with_options: load_sym!(device_library, "idevice_new_with_options"),
        idevice_free: load_sym!(device_library, "idevice_free"),
        idevice_get_udid: load_sym!(device_library, "idevice_get_udid"),
        idevice_connect: load_sym!(device_library, "idevice_connect"),
        idevice_disconnect: load_sym!(device_library, "idevice_disconnect"),
        idevice_connection_send: load_sym!(device_library, "idevice_connection_send"),
        idevice_connection_receive_timeout: load_sym!(
            device_library,
            "idevice_connection_receive_timeout"
        ),
        idevice_connection_enable_ssl: load_sym!(device_library, "idevice_connection_enable_ssl"),
        lockdownd_client_new: load_sym!(device_library, "lockdownd_client_new"),
        lockdownd_client_free: load_sym!(device_library, "lockdownd_client_free"),
        lockdownd_start_session: load_sym!(device_library, "lockdownd_start_session"),
        lockdownd_get_value: load_sym!(device_library, "lockdownd_get_value"),
        lockdownd_start_service: load_sym!(device_library, "lockdownd_start_service"),
        lockdownd_service_descriptor_free: load_sym!(
            device_library,
            "lockdownd_service_descriptor_free"
        ),
        afc_client_new: load_sym!(device_library, "afc_client_new"),
        afc_client_free: load_sym!(device_library, "afc_client_free"),
        afc_read_directory: load_sym!(device_library, "afc_read_directory"),
        afc_get_file_info: load_sym!(device_library, "afc_get_file_info"),
        afc_dictionary_free: load_sym!(device_library, "afc_dictionary_free"),
        afc_file_open: load_sym!(device_library, "afc_file_open"),
        afc_file_close: load_sym!(device_library, "afc_file_close"),
        afc_file_read: load_sym!(device_library, "afc_file_read"),
        afc_file_write: load_sym!(device_library, "afc_file_write"),
        afc_make_directory: load_sym!(device_library, "afc_make_directory"),
        afc_remove_path: load_sym!(device_library, "afc_remove_path"),
        plist_to_bin: load_sym!(plist_library, "plist_to_bin"),
        plist_free: load_sym!(plist_library, "plist_free"),
        plist_mem_free: load_sym!(plist_library, "plist_mem_free"),
        usbmuxd_read_pair_record: load_sym!(usbmuxd_library, "usbmuxd_read_pair_record"),
        _device_library: device_library,
        _plist_library: plist_library,
        _usbmuxd_library: usbmuxd_library,
        source,
    })
}

fn open_first(names: &[&str]) -> Result<(Library, String)> {
    let mut last_error = None;
    for name in names {
        // Loading a shared library runs its initialisers, so this stays in an
        // explicitly unsafe block rather than widening the whole function.
        match unsafe { Library::new(name) } {
            Ok(library) => return Ok((library, (*name).to_string())),
            Err(err) => last_error = Some(err.to_string()),
        }
    }

    match last_error {
        Some(err) => bail!("Could not load {}: {}", names.join(" or "), err),
        None => bail!("No library name to load"),
    }
}

pub fn verify_support() -> Result<String> {
    let libraries = libraries()?;
    Ok(format!(
        "libimobiledevice ready: {} (connect and unlock a paired iPhone)",
        libraries.source
    ))
}
