//! Apple Books sync over the AirTraffic (`com.apple.atc`) service.
//!
//! The handshake is `SyncAllowed` → `HostInfo` (session 0) → `RequestingSync`
//! (session 1) → `ReadyForSync` → `FinishedSyncingMetadata` → `AssetManifest` →
//! `FileComplete`. Two messages decide whether the sync lives or dies: every
//! `Ping` must be answered with `Pong`, and `SyncFailed` is a stale cancel from
//! an earlier session that must be ignored rather than treated as fatal.

use std::collections::HashMap;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::backend::{AtConnection, AtMessage, open_air_traffic, random_bytes};
use crate::device::{DeviceTransport, ensure_transport_available};
use crate::grappa;

/// How long one poll of the device message queue may wait.
const MESSAGE_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// How long a single receive may wait while a step is in progress.
const MESSAGE_TIMEOUT: Duration = Duration::from_secs(2);

/// Tries per handshake step. WiFi is slower, so `retry_scale` multiplies these.
const SYNC_ALLOWED_TRIES: u32 = 12;
const READY_FOR_SYNC_TRIES: u32 = 24;
const ASSET_MANIFEST_TRIES: u32 = 20;

pub enum SyncEvent {
    Log(String),
    Done(Result<()>),
}

pub fn sync_assets_via_airtraffic<L>(
    udid: &str,
    transport: DeviceTransport,
    assets: &[(&str, &str)],
    mut log: L,
) -> Result<()>
where
    L: FnMut(&str),
{
    let udid_owned = udid.to_string();
    let assets_owned: Vec<(String, String)> = assets
        .iter()
        .map(|(ident, destination)| (ident.to_string(), destination.to_string()))
        .collect();

    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::spawn(move || {
        let refs: Vec<(&str, &str)> = assets_owned
            .iter()
            .map(|(ident, destination)| (ident.as_str(), destination.as_str()))
            .collect();
        let tx_log = tx.clone();
        let res = sync_assets_via_airtraffic_internal(&udid_owned, transport, &refs, move |msg| {
            let _ = tx_log.send(SyncEvent::Log(msg.to_string()));
        });
        let _ = tx.send(SyncEvent::Done(res));
    });

    let base_timeout_secs = if transport == DeviceTransport::Wifi {
        120
    } else {
        60
    };
    let total_timeout_secs = base_timeout_secs.max(assets.len() as u64 * 2);
    let start = std::time::Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= Duration::from_secs(total_timeout_secs) {
            bail!("{}", timeout_advice(total_timeout_secs));
        }
        let timeout = Duration::from_secs(total_timeout_secs) - elapsed;
        match rx.recv_timeout(timeout) {
            Ok(SyncEvent::Log(msg)) => log(&msg),
            Ok(SyncEvent::Done(res)) => return res,
            Err(_) => bail!("{}", timeout_advice(total_timeout_secs)),
        }
    }
}

fn timeout_advice(total_timeout_secs: u64) -> String {
    let competing_client = if cfg!(windows) {
        "Close iTunes or Apple Devices on the computer."
    } else {
        "Close any other client holding the iPhone, such as idevicesyslog or a file manager."
    };
    format!(
        "AirTraffic sync timed out ({}s). 1) Unlock the iPhone and keep the screen on. 2) Open Apple Books on the iPhone once. 3) {}",
        total_timeout_secs, competing_client
    )
}

fn sync_assets_via_airtraffic_internal<L>(
    udid: &str,
    transport: DeviceTransport,
    assets: &[(&str, &str)],
    mut log: L,
) -> Result<()>
where
    L: FnMut(&str),
{
    ensure_transport_available(udid, transport)
        .context("The selected device transport disappeared before the AirTraffic sync")?;
    log(&format!(
        "Connecting to the iOS AirTraffic service (com.apple.atc) over {}...",
        transport.label()
    ));

    let conn = open_air_traffic(udid, transport).context("Failed to open the AirTraffic host connection")?;

    let mut sync = BooksSync {
        conn,
        retry_scale: if transport == DeviceTransport::Wifi { 2 } else { 1 },
        log: &mut log,
    };

    sync.run(assets)
}

/// One Books sync conversation with the device's AirTraffic daemon.
struct BooksSync<'a, L: FnMut(&str)> {
    conn: Box<dyn AtConnection>,
    retry_scale: u32,
    log: &'a mut L,
}

impl<L: FnMut(&str)> BooksSync<'_, L> {
    fn run(&mut self, assets: &[(&str, &str)]) -> Result<()> {
        self.wait_for_sync_allowed()?;

        self.log("SyncAllowed received. Handshaking the Books sync request...");
        let host_info = host_info_plist();
        self.conn.send_host_info(&host_info)?;
        sleep(Duration::from_millis(200));

        self.conn
            .send_sync_request(&book_dataclasses_plist(), &empty_anchors_plist(), &host_info)?;

        self.log("Waiting for ReadyForSync from the iPhone...");
        self.wait_for_ready_for_sync()?;

        self.conn
            .send_metadata_sync_finished(&book_sync_types_plist(), &empty_anchors_plist())?;

        let manifest = self.read_asset_manifest()?;
        let downloads = downloadable_assets(&manifest)?;
        ensure_assets_available(&downloads, assets)?;

        self.dispatch_asset_completed(assets)
    }

    fn wait_for_sync_allowed(&mut self) -> Result<()> {
        self.log("Waiting for SyncAllowed from the iPhone (keep the screen unlocked)...");

        for _ in 0..(SYNC_ALLOWED_TRIES * self.retry_scale) {
            let Some(message) = self.poll_message()? else {
                continue;
            };
            match message.name.as_str() {
                // The device drops the connection when a Ping goes unanswered,
                // and the Windows AirTraffic library used to hide this.
                "Ping" => {
                    self.conn.send_pong()?;
                    continue;
                }
                "SyncAllowed" => return Ok(()),
                _ => {}
            }
            self.log(&format!("AirTraffic message: {}", message.name));
        }

        bail!(
            "AirTraffic: SyncAllowed was not received. Unlock the iPhone screen and open the Books app once."
        )
    }

    fn wait_for_ready_for_sync(&mut self) -> Result<()> {
        for _ in 0..(READY_FOR_SYNC_TRIES * self.retry_scale) {
            let Some(message) = self.read_until(&["ReadyForSync", "AssetManifest"])? else {
                continue;
            };
            if message.name == "ReadyForSync" {
                return Ok(());
            }
        }

        bail!("AirTraffic: ReadyForSync was not received from the device")
    }

    fn read_asset_manifest(&mut self) -> Result<plist::Value> {
        for _ in 0..(ASSET_MANIFEST_TRIES * self.retry_scale) {
            let Some(message) = self.read_until(&["AssetManifest"])? else {
                continue;
            };
            return message
                .param("AssetManifest")
                .context("AirTraffic: the AssetManifest message carried no manifest");
        }

        bail!("AirTraffic: AssetManifest was not received or failed to parse")
    }

    fn dispatch_asset_completed(&mut self, assets: &[(&str, &str)]) -> Result<()> {
        for (index, (ident, destination)) in assets.iter().enumerate() {
            self.log(&format!("AirTraffic: reporting {} as downloaded", ident));
            self.conn.send_asset_completed(ident, "Book", destination)?;

            if index + 1 < assets.len() {
                sleep(Duration::from_millis(if index == 0 { 400 } else { 60 }));
            }
        }

        sleep(Duration::from_millis(2000));
        Ok(())
    }

    /// Reads messages until one of `wanted` arrives.
    ///
    /// Answers every `Ping` with `Pong`, skips `SyncFailed` (a stale cancel from
    /// an earlier session) and treats `SyncFinished` as the end of the session.
    fn read_until(&mut self, wanted: &[&str]) -> Result<Option<AtMessage>> {
        loop {
            let Some(message) = self.poll_message()? else {
                return Ok(None);
            };

            match message.name.as_str() {
                "Ping" => {
                    self.conn.send_pong()?;
                    continue;
                }
                "SyncFailed" => continue,
                "SyncFinished" => return Ok(None),
                _ => {}
            }

            if wanted.contains(&message.name.as_str()) {
                return Ok(Some(message));
            }
            self.log(&format!("AirTraffic message: {}", message.name));
        }
    }

    /// Reads one message, waiting one poll interval when the queue is empty.
    fn poll_message(&mut self) -> Result<Option<AtMessage>> {
        match self.conn.read_message(MESSAGE_TIMEOUT)? {
            Some(message) => Ok(Some(message)),
            None => {
                sleep(MESSAGE_POLL_INTERVAL);
                Ok(None)
            }
        }
    }

    fn log(&mut self, message: &str) {
        (self.log)(message);
    }
}

fn host_info_plist() -> plist::Value {
    let mut host_info = HashMap::new();
    host_info.insert("Type".to_string(), plist::Value::String("iTunes".to_string()));
    host_info.insert(
        "Version".to_string(),
        plist::Value::String("13.7.0.161".to_string()),
    );
    host_info.insert(
        "MacOSVersion".to_string(),
        plist::Value::String(host_platform().to_string()),
    );
    host_info.insert(
        "SyncHostName".to_string(),
        plist::Value::String("airlift".to_string()),
    );
    host_info.insert(
        "LibraryID".to_string(),
        plist::Value::String(generate_uuid_v4()),
    );
    host_info.insert(
        "SyncedDataclasses".to_string(),
        plist::Value::Array(vec![plist::Value::String("Book".to_string())]),
    );
    host_info.insert(
        "SyncedAssetTypes".to_string(),
        plist::Value::Array(vec![plist::Value::String("Book".to_string())]),
    );
    host_info.insert("Wakeable".to_string(), plist::Value::Boolean(false));
    host_info.insert(
        "Grappa".to_string(),
        plist::Value::Data(grappa::token(0)),
    );

    plist::Value::Dictionary(host_info.into_iter().collect())
}

fn host_platform() -> &'static str {
    if cfg!(windows) {
        "Windows NT 10.0"
    } else {
        "Linux"
    }
}

fn book_dataclasses_plist() -> plist::Value {
    plist::Value::Array(vec![plist::Value::String("Book".to_string())])
}

fn book_sync_types_plist() -> plist::Value {
    let mut sync_types = HashMap::new();
    sync_types.insert("Book".to_string(), plist::Value::Integer(1.into()));
    plist::Value::Dictionary(sync_types.into_iter().collect())
}

fn empty_anchors_plist() -> plist::Value {
    plist::Value::Dictionary(HashMap::<String, plist::Value>::new().into_iter().collect())
}

/// Asset identifiers the device reports as available for download.
fn downloadable_assets(manifest: &plist::Value) -> Result<Vec<String>> {
    let book_entries = manifest
        .as_dictionary()
        .and_then(|dict| dict.get("Book"))
        .and_then(|value| value.as_array())
        .context("The AssetManifest does not contain a Book list")?;

    let mut downloads = Vec::new();
    for entry in book_entries {
        let Some(dict) = entry.as_dictionary() else {
            continue;
        };
        let is_download = dict
            .get("IsDownload")
            .and_then(|value| value.as_boolean())
            .unwrap_or(false);
        if !is_download {
            continue;
        }
        if let Some(asset_id) = dict.get("AssetID").and_then(|value| value.as_string()) {
            downloads.push(asset_id.to_string());
        }
    }

    Ok(downloads)
}

/// Every requested asset must appear in the device's download manifest.
fn ensure_assets_available(downloads: &[String], assets: &[(&str, &str)]) -> Result<()> {
    for (ident, _) in assets {
        if !downloads.iter().any(|download| download == ident) {
            bail!(
                "Asset '{}' is missing from the device download manifest (available: {:?})",
                ident,
                downloads
            );
        }
    }
    Ok(())
}

fn generate_uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    let _ = random_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_downloadable_assets_filters_manifest() {
        let mut downloadable = HashMap::new();
        downloadable.insert("AssetID".to_string(), plist::Value::String("wanted".to_string()));
        downloadable.insert("IsDownload".to_string(), plist::Value::Boolean(true));

        let mut installed = HashMap::new();
        installed.insert("AssetID".to_string(), plist::Value::String("owned".to_string()));
        installed.insert("IsDownload".to_string(), plist::Value::Boolean(false));

        let mut manifest = HashMap::new();
        manifest.insert(
            "Book".to_string(),
            plist::Value::Array(vec![
                plist::Value::Dictionary(downloadable.into_iter().collect()),
                plist::Value::Dictionary(installed.into_iter().collect()),
            ]),
        );

        let root = plist::Value::Dictionary(manifest.into_iter().collect());
        assert_eq!(downloadable_assets(&root).unwrap(), vec!["wanted".to_string()]);
    }

    #[test]
    fn test_asset_availability_rejects_missing_asset() {
        let downloads = vec!["present".to_string()];
        assert!(ensure_assets_available(&downloads, &[("present", "/a")]).is_ok());
        assert!(ensure_assets_available(&downloads, &[("absent", "/b")]).is_err());
    }

    #[test]
    fn test_host_info_carries_grappa_token() {
        let host_info = host_info_plist();
        let dict = host_info.as_dictionary().expect("host info is a dictionary");
        let grappa = dict
            .get("Grappa")
            .and_then(|value| value.as_data())
            .expect("host info carries a Grappa token");
        assert_eq!(grappa.len(), 84);
    }
}
