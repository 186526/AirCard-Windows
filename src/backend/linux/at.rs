//! AirTraffic (`com.apple.atc`) host connection over libimobiledevice.
//!
//! ATC frames carry a 4-byte **little-endian** length prefix followed by a
//! binary property list with the envelope `{ Command, Session, Params }`. This
//! is a different framing from `com.apple.streaming_zip_conduit`, which uses a
//! big-endian prefix; the two services must not share a frame helper.

use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::backend::{AtConnection, AtMessage, Received, ServiceConnection};

use super::session::LinuxServiceConnection;

/// Longest ATC frame body accepted from the device, in bytes.
const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Session number the device uses for the host identity message.
pub const SESSION_HOST_INFO: u64 = 0;
/// Session number the device uses for every other sync message.
pub const SESSION_SYNC: u64 = 1;

/// One ATC message, already decoded from the device's property list.
///
/// The Linux transport decodes the whole frame before handing it to the caller,
/// so unlike the Windows backend there is no library object left to release.
pub struct MessageHandle {
    params: Option<plist::Dictionary>,
}

impl MessageHandle {
    pub fn param(&self, key: &str) -> Option<plist::Value> {
        self.params.as_ref()?.get(key).cloned()
    }
}

pub struct LinuxAtConnection {
    session: LinuxServiceConnection,
}

impl LinuxAtConnection {
    pub(super) fn new(session: LinuxServiceConnection) -> Self {
        Self { session }
    }

    /// Sends one message with the little-endian ATC framing.
    fn send(&self, command: &str, session: u64, params: Option<plist::Dictionary>) -> Result<()> {
        self.session.send_all(&encode_message(command, session, params)?)
    }

    /// Reads the four byte little-endian length of the next ATC frame.
    fn read_frame_length(&self, timeout: Duration) -> Result<Option<usize>> {
        let mut header = [0u8; 4];
        match self.session.receive_exact(&mut header, timeout)? {
            Received::Idle => return Ok(None),
            Received::Closed => bail!("The device closed the AirTraffic connection"),
            Received::Data { .. } => {}
        }

        // ATC is little-endian by definition; a value that cannot describe a
        // frame means the stream is out of sync, not that the byte order changed.
        let length = u32::from_le_bytes(header) as usize;
        if length == 0 || length > MAX_FRAME {
            bail!(
                "No plausible AirTraffic frame length in header {:02x?} ({})",
                header,
                length
            );
        }

        Ok(Some(length))
    }
}

impl AtConnection for LinuxAtConnection {
    fn read_message(&self, timeout: Duration) -> Result<Option<AtMessage>> {
        let Some(length) = self.read_frame_length(timeout)? else {
            return Ok(None);
        };

        let mut body = vec![0u8; length];
        self.session.receive_body(&mut body, timeout)?;

        let value = plist::Value::from_reader(std::io::Cursor::new(body))
            .context("Failed to decode an AirTraffic message")?;
        let dictionary = value
            .as_dictionary()
            .context("AirTraffic sent a message that is not a dictionary")?;

        let name = dictionary
            .get("Command")
            .and_then(|value| value.as_string())
            .context("AirTraffic message carries no Command")?
            .to_string();
        let params = dictionary
            .get("Params")
            .and_then(|value| value.as_dictionary())
            .cloned();

        Ok(Some(AtMessage::new(name, MessageHandle { params })))
    }

    fn send_host_info(&self, host_info: &plist::Value) -> Result<()> {
        let mut params = plist::Dictionary::new();
        params.insert("HostInfo".to_string(), host_info.clone());
        params.insert("LocalCloudSupport".to_string(), plist::Value::Boolean(false));
        self.send("HostInfo", SESSION_HOST_INFO, Some(params))
    }

    fn send_pong(&self) -> Result<()> {
        self.send("Pong", SESSION_SYNC, None)
    }

    fn send_sync_request(
        &self,
        dataclasses: &plist::Value,
        anchors: &plist::Value,
        host_info: &plist::Value,
    ) -> Result<()> {
        let mut params = plist::Dictionary::new();
        params.insert("Dataclasses".to_string(), dataclasses.clone());
        params.insert("DataclassAnchors".to_string(), anchors.clone());
        params.insert("HostInfo".to_string(), host_info.clone());
        if let Some(grappa) = host_info.as_dictionary().and_then(|dict| dict.get("Grappa")) {
            params.insert("Grappa".to_string(), grappa.clone());
        }
        self.send("RequestingSync", SESSION_SYNC, Some(params))
    }

    fn send_metadata_sync_finished(
        &self,
        sync_types: &plist::Value,
        anchors: &plist::Value,
    ) -> Result<()> {
        let mut params = plist::Dictionary::new();
        params.insert("SyncTypes".to_string(), sync_types.clone());
        params.insert("DataclassAnchors".to_string(), anchors.clone());
        self.send("FinishedSyncingMetadata", SESSION_SYNC, Some(params))
    }

    fn send_asset_completed(&self, asset_id: &str, dataclass: &str, destination: &str) -> Result<()> {
        let mut params = plist::Dictionary::new();
        params.insert("AssetID".to_string(), plist::Value::String(asset_id.to_string()));
        params.insert("Dataclass".to_string(), plist::Value::String(dataclass.to_string()));
        params.insert("AssetPath".to_string(), plist::Value::String(destination.to_string()));
        self.send("FileComplete", SESSION_SYNC, Some(params))
    }
}

/// Encodes one ATC message with its 4-byte little-endian length prefix.
fn encode_message(
    command: &str,
    session: u64,
    params: Option<plist::Dictionary>,
) -> Result<Vec<u8>> {
    let mut envelope = plist::Dictionary::new();
    envelope.insert("Command".to_string(), plist::Value::String(command.to_string()));
    envelope.insert("Session".to_string(), plist::Value::Integer(session.into()));
    if let Some(params) = params {
        envelope.insert("Params".to_string(), plist::Value::Dictionary(params));
    }

    let mut body = Vec::new();
    plist::to_writer_binary(&mut body, &plist::Value::Dictionary(envelope))
        .context("Failed to encode an AirTraffic message")?;

    let mut framed = Vec::with_capacity(body.len() + 4);
    framed.extend_from_slice(&(body.len() as u32).to_le_bytes());
    framed.extend_from_slice(&body);
    Ok(framed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ATC length prefix is little-endian. Reading it back as big-endian
    /// yields a value far too large to be a frame, which is the failure this
    /// guards against.
    #[test]
    fn test_atc_length_prefix_is_little_endian() {
        let framed = encode_message("Pong", SESSION_SYNC, None).unwrap();
        let declared = u32::from_le_bytes(framed[..4].try_into().unwrap()) as usize;

        assert_eq!(declared, framed.len() - 4);
        assert!(u32::from_be_bytes(framed[..4].try_into().unwrap()) as usize > (16 * 1024 * 1024));
    }

    #[test]
    fn test_atc_envelope_carries_command_and_session() {
        let framed = encode_message("SyncAllowed", SESSION_SYNC, None).unwrap();
        let value = plist::Value::from_reader(std::io::Cursor::new(&framed[4..])).unwrap();
        let envelope = value.as_dictionary().unwrap();

        assert_eq!(
            envelope.get("Command").and_then(|value| value.as_string()),
            Some("SyncAllowed")
        );
        assert_eq!(
            envelope.get("Session").and_then(|value| value.as_signed_integer()),
            Some(SESSION_SYNC as i64)
        );
    }
}
