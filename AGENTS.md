# AGENTS.md

AirCard is a native desktop app (Rust + egui/eframe) that customizes Apple Wallet card artwork and iOS lockscreen passcode keypad themes on a connected iPhone, without jailbreak. It runs on Windows and on Linux. Both builds use the "airlift" AirTraffic sandbox-escape chain to write files outside the iOS app sandbox.

Documentation and comments in this repository are written in English. Keep new code, comments, log messages and UI strings in English.

## Build and run

Both platforms are supported from the same tree; the device backend is selected by `cfg`, not by a feature flag. Windows targets `stable-x86_64-pc-windows-msvc`, Linux targets `stable-x86_64-unknown-linux-gnu`; edition 2024 (Rust 1.85 or newer).

```bash
cargo test            # unit tests in src/*.rs + tests/engine_tests.rs
cargo check --all-targets
cargo build           # debug build
cargo build --release # target/release/aircard[.exe]
```

Neither build links against the support stack: the Windows build loads Apple's DLLs and the Linux build loads libimobiledevice / libplist / libusbmuxd, both at run time through `libloading`, so `cargo build` succeeds on a machine with no iPhone and no support packages installed. On Linux `usbmuxd` must be running and the three runtime libraries must be present; `src/backend/linux/ffi.rs` holds the soname candidates it tries (`libimobiledevice-1.0.so.6`; `libplist-2.0.so.4` then `.so.3`; `libusbmuxd-2.0.so.7` then `.so.6`; then the unversioned name), so no `-dev` package is needed.

The release profile in `Cargo.toml` sets `lto = true`, `codegen-units = 1`, `strip = "symbols"`. `src/main.rs` sets `windows_subsystem = "windows"` outside debug builds, so Windows release builds have no console.

Windows requires 64-bit iTunes / Apple Mobile Device Support and a USB-paired, unlocked iPhone. Linux requires `usbmuxd` plus the libimobiledevice stack. On both platforms USB pairing must be done first; WiFi transport only works after that.

The version lives in `Cargo.toml` only. `src/main.rs` derives `APP_TITLE` from it with `concat!("AirCard v", env!("CARGO_PKG_VERSION"))`, and the window title, the header label, the startup log line and the app header all use that value or `env!("CARGO_PKG_VERSION")` directly.

Versions in use: `eframe`/`egui` 0.33, `image` 0.25, `zip` 2 (deflate only), `libloading` 0.8, `plist` 1, `rfd` 0.17, `serde`/`serde_json`, `anyhow`, `regex`, `base64`, `miniz_oxide`, `libc` (unix only). There is no `Cargo.toml` `[dev-dependencies]` section; tests use the standard library only. The repo has no `rustfmt.toml`, `clippy.toml`, or `rust-toolchain.toml` — default rustfmt formatting applies.

## Module map

Platform-independent layers:

- `src/main.rs` — window setup (`960x620`, min `850x560`), module list, `APP_TITLE`, `eframe::run_native`.
- `src/backend/mod.rs` — the seam. Declares `ServiceConnection`, `DeviceSession`, `Afc`, `AtConnection` and `AtMessage`, re-exports the platform backend's `list_usbmux_devices`, `open_session`, `open_air_traffic`, `read_device_identity`, `verify_support` and `random_bytes`, and holds the `#[cfg]` selection of `src/backend/windows` versus `src/backend/linux`.
- `src/device.rs` — `DeviceInfo` merging across transports, `DeviceTransport` (USB/WiFi/Other), `ConnectionMode` (Auto/USB/WiFi), `ordered_candidates`, and `open_active_session` (pick a candidate → `open_session`).
- `src/airtraffic.rs` — the `com.apple.atc` host handshake (`SyncAllowed` → `HostInfo` → `SyncRequest` → `ReadyForSync` → `MetadataSyncFinished` → `AssetManifest` → `AssetCompleted`) and `sync_assets_via_airtraffic`, which runs the sync on a worker thread and enforces a 60 s (USB) / 120 s (WiFi) timeout. The host plist's `Type` is deliberately `"iTunes"`: the device expects that identity.
- `src/grappa.rs` — ten pre-generated 84-byte Grappa client tokens for `(version = 1, deviceType = 0, protocolVersion = 1)`. The device advertises `GrappaSupportInfo` and silently drops the sync when `HostInfo` / `RequestingSync` carry no matching token.
- `src/airlift.rs` — exploit primitives: `build_streaming_zip_archive[_multi]` (stored zip with Apple's `0x5A53` Unix-mode extra field and a `p0/p1/p2/link` symlink pointing at the target path), `build_books_plist`, `snapshot_books`/`restore_books` over `TRACKED_BOOKS_FILES`, and `stage_streaming_zip` (sends the archive to `com.apple.streaming_zip_conduit`).
- `src/flasher.rs` — orchestration: `generate_token` (10 random bytes through `backend::random_bytes`, hex), `write_system_file`, `write_system_files_batch` (atomic multi-asset flash in one session), `flash_wallet_skin`, `flash_passcode_theme`. Every write snapshots the tracked Books files first and restores them in cleanup.
- `src/image_skin.rs` — center-crops and Lanczos-resizes to `1536x969`, encodes PNG, and hand-builds a single-page PDF (`%PDF-1.4`, FlateDecode image XObject) with no PDF library.
- `src/passthm.rs` — parses `.passthm` (zip) theme packages into `(target_dir, leaf_name, data)` items plus per-digit preview images. Emits `_big` marker files and `<lang>-<digit>[-<subtext>]--white[-bold].png` variants; layouts for English, Russian, Ukrainian and Japanese subtexts.
- `src/scanner.rs` — listens on `com.apple.syslog_relay` (500 ms socket receive timeout), extracts card hashes and names with regexes, validates them heuristically, and persists them to `cards.json` under `%LOCALAPPDATA%\AirCard` on Windows or `$XDG_DATA_HOME/aircard` (falling back to `~/.local/share/aircard`) on Linux.
- `src/app.rs` — the whole UI: MD3 dark palette (`pub mod md3`), `m3_card` / `m3_button_*` helpers, tabs `Wallet` / `Passcode` / `Help`, a log window, and the background-task plumbing.

The two backends, which must satisfy the same traits:

- `src/backend/windows/` — `ffi.rs` loads `CoreFoundation.dll`, `MobileDevice.dll`, `AirTrafficHost.dll` from `C:\Program Files\Common Files\Apple\Mobile Device Support` (or the `x86` variant) via `libloading`, after `SetDllDirectoryW`, and holds every FFI symbol plus the `CFStringGuard` / `CFTypeGuard` RAII wrappers; `misc.rs` wraps `BCryptGenRandom` and `setsockopt`; `session.rs`, `afc.rs`, `at.rs` and `usbmux.rs` implement the traits.
- `src/backend/linux/` — `ffi.rs` opens the three libraries by soname and declares every symbol; `pairing.rs` reads the existing usbmuxd pair record; `plist_bridge.rs` converts between `plist_t` and `plist::Value` (`PlistHandle` owns one node tree and frees it on drop; `from_raw` adopts a pointer the library returned); `session.rs` implements lockdown plus the `com.apple.streaming_zip_conduit` big-endian framing; `afc.rs` and `at.rs` implement the AFC and ATC traits; `usbmux.rs` enumerates devices.

## Architecture conventions

- Device work runs on `std::thread`, never on the UI thread. The UI holds a `Receiver<BackgroundTaskMessage>` and drains it with `try_recv` in `AirCardApp::handle_messages` (called at the top of `update`), calling `ctx.request_repaint()` while `is_busy || scanning_syslog`.
- Cancellable loops (syslog scan) take `Arc<AtomicBool>` stop flags. Long device operations take progress and log callbacks: `F: FnMut(usize, usize, &str)` and `L: FnMut(&str)`.
- Errors use `anyhow`: `bail!` for failures, `.context(...)` for layering, and `{err:#}` when formatting a chained error into a log or status line. User-facing failure text goes through `status_msg` / logs, not panics.
- Everything platform-specific about talking to a device belongs behind the traits in `src/backend/mod.rs`; nothing above that seam may name an Apple DLL, an `AFC*` symbol or a libimobiledevice function. When you need a new device capability, add it to the trait and implement it in both backends.
- Interop is raw FFI. On Windows every `AFC*` / `AMDevice*` / `AT*` symbol is declared in the `AppleLibraries` struct in `src/backend/windows/ffi.rs`; on Linux every `afc_*` / `lockdownd_*` / `idevice_*` / `plist_*` / `usbmuxd_*` symbol is declared in `src/backend/linux/ffi.rs`. Add new symbols there rather than declaring local `extern` blocks. Wrap CoreFoundation objects in `CFStringGuard` / `CFTypeGuard` and `plist_t` values in `OwnedPlist` instead of releasing by hand. Buffer sizes are checked and the code bails on non-zero status codes; keep that pattern.
- The two services do not share a frame format: `com.apple.atc` prefixes a 4-byte little-endian length, `com.apple.streaming_zip_conduit` a 4-byte big-endian one. Keep the two helpers separate; do not factor them into one.
- File writes to the device are staged, not written directly: the payload travels inside the streaming-zip source directory and reaches its destination through the `p0/p1/p2/link` symlink during the AirTraffic Books sync. `src/flasher.rs` deletes the source/link/recovered objects and restores the Books snapshot in cleanup even when the write fails — preserve that ordering.
- UI strings are user-facing and English: short, action-oriented button labels (`Apply Card Skin`, `Choose Image...`), no explanation of internal design. Keep the same wording across tabs, README and logs. Where the host runtime differs per platform, branch the wording on `cfg!(windows)` instead of naming iTunes on Linux or libimobiledevice on Windows.

## Testing

- Unit tests live in `#[cfg(test)] mod tests` inside `src/airlift.rs`, `src/airtraffic.rs`, `src/device.rs`, `src/flasher.rs`, `src/grappa.rs`, `src/image_skin.rs`, `src/passthm.rs`, `src/scanner.rs`, and `src/backend/linux/{at,pairing}.rs`, plus `tests/engine_tests.rs`.
- `.github/workflows/build.yml` runs on push/PR to `main`/`master` and on `v*` tags, with one job per platform: `build-windows` on `windows-latest` and `build-linux` on `ubuntu-latest`. Each runs `cargo test` before its release build and uploads `aircard.exe` or `aircard` as an artifact; a `v*` tag attaches both to a GitHub Release. The Linux job installs the three libimobiledevice sonames plus `libegl1 libgl1 libxkbcommon0 libwayland-client0` before testing, because the tests dlopen them.
- `cargo check` only compiles the backend for the host platform, so a change under `src/backend/windows/` is **not** verified by running `cargo check` on Linux. Cross-check it with `cargo check --target x86_64-pc-windows-msvc`; that needs the target's `rust-std` (`rustup target add x86_64-pc-windows-msvc`) and nothing else, because the code dlopens Apple's DLLs at run time and never links them. Reviewing the diff is not a substitute — a private item referenced from a sibling module, for instance, is a hard error the Linux build cannot see.
- Pure-logic tests (hash validation, hash/name extraction, candidate ordering, device merging, zip/plist building, PNG→PDF, `.passthm` parsing, Grappa token length, ATC framing, pair-record decoding) must stay hardware-independent. `parse_passthm_file` tests build a synthetic zip in `std::env::temp_dir()` — follow that pattern for new theme parsing tests.
- Device-dependent tests (`test_usbmux_query`, `test_list_connected_devices`, `test_syslog_service_receive`) deliberately treat a missing usbmuxd, missing support libraries or an absent iPhone as a printed skip, not a failure, because CI has no device. Keep that behaviour so `cargo test` stays green on a machine without an iPhone.
- `tests/engine_tests.rs` starts with `#![cfg(windows)]` and calls `BCryptGenRandom` through a `#[link(name = "bcrypt")]` block, so on Linux it compiles to an empty test binary — a passing `cargo test` on Linux does not exercise it.
- Run a focused test with `cargo test <name>`; there is no separate lint or format gate in CI.

## Security and safety notes

- The app intentionally performs an undocumented write outside the iOS app sandbox by abusing the AirTraffic Books sync conduit. Do not "fix" this into a supported API path or add capability checks that would break the write flow.
- **Never pair from the Linux backend.** `lockdownd_client_new_with_handshake` auto-pairs when no record exists, and `lockdownd_pair` pairs unconditionally; both write a trusted-host entry on the user's physical device as a side effect of merely connecting. `src/backend/linux/pairing.rs` reads the record `usbmuxd` already holds and the session then calls `lockdownd_start_session` directly. An unpaired device must fail with a clear message asking the user to trust the computer, not be paired silently.
- Anything that connects to a device can modify it: `write_system_file(s)`, `flash_wallet_skin` and `flash_passcode_theme` write real files on a real iPhone. Do not run them against a user's device as part of a build, test or routine verification; ask the user first.
- The Books snapshot/restore in `src/airlift.rs` (`snapshot_books` / `restore_books`) is the only rollback mechanism. Do not remove or reorder it, and do not skip cleanup on error paths.
- `cards.json` stores card hashes in clear text. No other user data is collected and there is no network or telemetry code.
- UI copy for Bold Text and WiFi mode reflects hard device constraints (Bold Text ON makes iOS ignore cached keypad graphics; AirTraffic selects its route by UDID, so a true WiFi path needs the cable unplugged). Do not reword these as generic advice.
