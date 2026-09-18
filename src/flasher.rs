use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use std::collections::HashMap;

use crate::afc::AfcClient;
use crate::airlift::{
    LINK_PREFIX, RECOVERED_PREFIX, SOURCE_PREFIX, build_books_plist,
    build_streaming_zip_archive_multi, restore_books, snapshot_books,
    stage_streaming_zip,
};
use crate::airtraffic::sync_assets_via_airtraffic;
use crate::device::ActiveDeviceSession;

pub const TARGET_WALLET_ASSETS: &[&str] = &[
    "cardBackgroundCombined@3x.png",
    "cardBackgroundCombined@2x.png",
    "cardBackgroundCombined.png",
    "cardBackgroundCombined1@3x.png",
    "cardBackgroundCombined1@2x.png",
    "cardBackgroundCombined1.png",
    "cardBackground@3x.png",
    "cardBackground@2x.png",
    "cardBackground.png",
    "background@3x.png",
    "background@2x.png",
    "background.png",
    "FrontFace@3x.png",
    "FrontFace@2x.png",
    "FrontFace.png",
    "FrontFace",
    "Preview@3x.png",
    "Preview@2x.png",
    "Preview.png",
    "Preview",
];

pub const CACHE_FILES: &[&str] = &[
    "FrontFace",
    "FrontFace.png",
    "FrontFace@2x",
    "FrontFace@2x.png",
    "FrontFace@3x",
    "FrontFace@3x.png",
    "Preview",
    "Preview.png",
    "Preview@2x",
    "Preview@2x.png",
    "Preview@3x",
    "Preview@3x.png",
    "PlaceHolder",
    "PlaceHolder.png",
    "PlaceHolder@2x",
    "PlaceHolder@2x.png",
    "PlaceHolder@3x",
    "PlaceHolder@3x.png",
];

#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(
        hAlgorithm: *mut std::ffi::c_void,
        pbBuffer: *mut u8,
        cbBuffer: u32,
        dwFlags: u32,
    ) -> i32;
}

pub fn generate_token() -> String {
    let mut bytes = [0u8; 10];
    unsafe {
        let _ = BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            2, // BCRYPT_USE_SYSTEM_PREFERRED_RNG
        );
    }
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn write_system_files_batch<L>(
    udid: &str,
    target_dir: &str,
    items: &[(&str, &[u8])],
    mut log: L,
) -> Result<()>
where
    L: FnMut(&str),
{
    if items.is_empty() {
        return Ok(());
    }

    let token = generate_token();
    log(&format!("Staging token generated: {}", token));
    log(&format!("Target directory: {}", target_dir));
    log(&format!("Preparing batch of {} payload item(s)...", items.len()));

    let source = format!("{}{}", SOURCE_PREFIX, token);
    let link_dest = format!("{}{}", LINK_PREFIX, token);
    let recovered = format!("{}{}", RECOVERED_PREFIX, token);

    let link_ident = format!("../../{}/p0/p1/p2/link", source);

    let mut books_identifiers = Vec::with_capacity(items.len() + 1);
    books_identifiers.push(link_ident.clone());

    let mut assets_to_sync = Vec::with_capacity(items.len() + 1);
    assets_to_sync.push((link_ident, link_dest.clone()));

    for (idx, (leaf_name, _)) in items.iter().enumerate() {
        let payload_ident = format!("../../{}/payload_{}", source, idx);
        let dest = format!("{}/{}", link_dest, leaf_name);
        books_identifiers.push(payload_ident.clone());
        assets_to_sync.push((payload_ident, dest));
    }

    log("Connecting to device AFC (Apple File Conduit)...");
    let session = ActiveDeviceSession::open(Some(udid))
        .context("Failed to open device session for writing")?;
    let afc = AfcClient::new(&session).context("Failed to open AFC connection")?;

    log("Creating safety snapshot of Books sync state...");
    let snapshot = snapshot_books(&afc).context("Failed to snapshot Books state before staging")?;

    log("Assembling streaming zip archive with payload...");
    let archive_data = build_streaming_zip_archive_multi(target_dir, items)
        .context("Failed to build streaming zip archive")?;
    log(&format!("Streaming zip archive built: {} bytes", archive_data.len()));

    let books_plist = build_books_plist(&books_identifiers)
        .context("Failed to build Books.plist")?;

    let write_res = (|| -> Result<()> {
        log("Staging streaming zip conduit via MobileInstallation...");
        stage_streaming_zip(&session, &source, &archive_data)
            .context("Failed to stage streaming zip conduit")?;

        log("Validating staged source objects on AFC filesystem...");
        let link_obj = format!("{}/p0/p1/p2/link", source);
        if !afc.exists(&source) || !afc.exists(&link_obj) {
            bail!("StreamingZip completed but link object is missing on AFC filesystem");
        }
        for idx in 0..items.len() {
            let payload_obj = format!("{}/payload_{}", source, idx);
            if !afc.exists(&payload_obj) {
                bail!("StreamingZip payload_{} is missing on AFC filesystem", idx);
            }
        }

        log("Writing manifest Books/Sync/Books.plist...");
        afc.make_directory_recursive("Books/Sync")?;
        afc.write_file("Books/Sync/Books.plist", &books_plist)?;
        if !afc.exists("Books/Sync/Books.plist") {
            bail!("Failed to stage Books/Sync/Books.plist");
        }

        log(&format!("Initiating AirTraffic sync protocol ({} asset pairs)...", assets_to_sync.len()));
        let sync_refs: Vec<(&str, &str)> = assets_to_sync
            .iter()
            .map(|(id, dst)| (id.as_str(), dst.as_str()))
            .collect();
        sync_assets_via_airtraffic(udid, &sync_refs)
            .context("AirTraffic sync failed")?;
        log("AirTraffic sync completed successfully.");

        Ok(())
    })();

    log("Cleaning up temporary conduit staging objects on device...");
    let _ = afc.remove_path(&link_dest);
    let _ = afc.remove_path(&recovered);
    let _ = afc.remove_tree(&source);
    sleep(Duration::from_millis(1500));

    log("Restoring Books state snapshot...");
    let restore_res = restore_books(&afc, &snapshot);

    write_res?;
    restore_res.context("Failed to restore Books state during cleanup")?;
    log("Batch write operation completed.");

    Ok(())
}

#[allow(dead_code)]
pub fn write_system_file(
    udid: &str,
    target_dir: &str,
    leaf_name: &str,
    payload: &[u8],
) -> Result<()> {
    write_system_files_batch(udid, target_dir, &[(leaf_name, payload)], |_| {})
}

pub fn flash_wallet_skin<F, L>(
    udid: &str,
    card_hash: &str,
    skin_png: &[u8],
    mut progress: F,
    mut log: L,
) -> Result<()>
where
    F: FnMut(usize, usize, &str),
    L: FnMut(&str),
{
    let pkpass_dir = format!("/var/mobile/Library/Passes/Cards/{}.pkpass", card_hash);

    log(&format!("Target Card Hash: {}", card_hash));
    log(&format!("Skin payload size: {} bytes PNG", skin_png.len()));

    progress(1, 3, "Writing primary wallet card assets...");
    log(&format!("[1/3] Writing {} primary card assets to {}...", TARGET_WALLET_ASSETS.len(), pkpass_dir));
    let assets: Vec<(&str, &[u8])> = TARGET_WALLET_ASSETS
        .iter()
        .map(|&name| (name, skin_png))
        .collect();
    write_system_files_batch(udid, &pkpass_dir, &assets, &mut log)
        .context("Failed to write card assets")?;

    let cache_items: Vec<(&str, &[u8])> = CACHE_FILES
        .iter()
        .map(|&name| (name, skin_png))
        .collect();

    for (i, ext) in [".cache", ".pkcache"].iter().enumerate() {
        let cache_dir = format!("/var/mobile/Library/Passes/Cards/{}{}", card_hash, ext);
        progress(2 + i, 3, &format!("Updating card cache ({})...", ext));
        log(&format!("[{}/3] Updating cache in {} ({} files)...", 2 + i, cache_dir, cache_items.len()));
        let _ = write_system_files_batch(udid, &cache_dir, &cache_items, &mut log);
    }

    log("Card skin write finished! Close and reopen Wallet on iPhone to view.");
    Ok(())
}

pub fn flash_passcode_theme<F, L>(
    udid: &str,
    items: &[(String, String, Vec<u8>)],
    mut progress: F,
    mut log: L,
) -> Result<()>
where
    F: FnMut(usize, usize, &str),
    L: FnMut(&str),
{
    let mut by_dir: HashMap<&str, Vec<(&str, &[u8])>> = HashMap::new();
    for (target_dir, leaf, payload) in items {
        by_dir
            .entry(target_dir.as_str())
            .or_default()
            .push((leaf.as_str(), payload.as_slice()));
    }

    let total_dirs = by_dir.len();
    log(&format!("Flashing passcode theme across {} target directories...", total_dirs));
    for (idx, (target_dir, dir_items)) in by_dir.iter().enumerate() {
        progress(
            idx + 1,
            total_dirs,
            &format!("Writing {} button assets to {}...", dir_items.len(), target_dir),
        );
        log(&format!("[{}/{}] Writing {} assets to {}...", idx + 1, total_dirs, dir_items.len(), target_dir));
        write_system_files_batch(udid, target_dir, dir_items, &mut log)
            .context(format!("Failed to write passcode buttons to {}", target_dir))?;
    }

    log("Passcode theme successfully written! Lock iPhone to see new keypad.");
    Ok(())
}
