# AirCard 🎴

> **Apple Wallet Card Skinner & Lockscreen Passcode Themer for iOS 18+ (No Jailbreak Required)**  
> Native Windows and Linux client written in Rust. Powered by the `airlift` AirTraffic sync exploit.

---

## Features
- 🎨 **Custom Card Skins:** Assign custom artwork, textures, or bank logos to Apple Pay and Apple Cash cards.
- 🔢 **Lock Screen Passcode Themes (.passthm):** Apply custom keypad button artwork from popular Cowabunga & Nugget `.passthm` themes directly to iOS lockscreen.
- ⚡ **100% Native & Lightweight:** Single standalone binary (~12.5 MB on Linux, ~7.5 MB on Windows). No Python, no Flet, no webview, no bloated runtimes.
- 🪟 **Material Design 3 Interface:** Clean, modern dark theme built with `egui` and `eframe`.
- 📱 **Zero-Hassle Card Detection:** Tap any card in your iPhone's Wallet app while connected to detect its hash in real-time via `syslog_relay`.
- 📶 **USB & WiFi Transport:** Scan card events and apply Wallet or passcode assets through USB or a paired local WiFi connection.
- 🔄 **Safe & Reversible:** Complete Books state snapshot and automatic restore engine — preserves original device state.
- 🚀 **Zero Jailbreak:** Utilizes Apple's built-in AirTraffic sync conduit without modifying system partitions or disabling security.

---

## Requirements

**Windows**
- **Windows 10 / 11 (64-bit)**
- **Apple Mobile Device Support / 64-bit iTunes** (required for Apple device communication).

**Linux**
- A 64-bit distro with a desktop session (X11 or Wayland) for the `egui` window.
- The public libimobiledevice stack plus the `usbmuxd` daemon. Package names vary by release:
  - Debian / Ubuntu: `sudo apt install usbmuxd libimobiledevice-1.0-6 libplist-2.0-4 libusbmuxd-2.0-7`
    (on older releases the plist/usbmuxd packages are named `libplist3` / `libusbmuxd6`)
  - Fedora: `sudo dnf install usbmuxd libimobiledevice libplist libusbmuxd`
  - Arch: `sudo pacman -S usbmuxd libimobiledevice libplist libusbmuxd`
- The runtime libraries are loaded by soname (`libimobiledevice-1.0.so.6`, `libplist-2.0.so.4`, `libusbmuxd-2.0.so.7`), so no `-dev` package is needed to build or run. Make sure `usbmuxd` is running (`systemctl status usbmuxd`) before launching AirCard.

**Both platforms**
- A Lightning or USB-C cable for the initial trust/pairing setup.
- For WiFi mode, enable WiFi sync and keep the PC and iPhone on the same local network.

---

## ⚠️ Troubleshooting (If Nothing Works)

> [!TIP]
> **Windows — iPhone not detected, AirTraffic sync hangs, or operation fails?**  
> Corrupted or conflicting Apple USB drivers on Windows are the #1 root cause.
> 1. Download and install **[3uTools](https://www.3u.com/)**.
> 2. **Disconnect your iPhone** from your PC.
> 3. In 3uTools, go to **Toolbox ➔ Repair Driver**.
> 4. Click **Repair Now** and wait for the Apple driver reinstallation to finish.
> 5. Reconnect your unlocked iPhone, tap **Trust**, and launch **AirCard**.

> [!TIP]
> **Linux — the device list stays empty or says usbmuxd is unreachable?**
> 1. Plug in the iPhone and unlock it. On a stock install `usbmuxd` is started by a udev rule when an Apple device is attached, so it is normally *not* running while nothing is plugged in.
> 2. If an iPhone *is* connected, confirm the daemon came up: `systemctl status usbmuxd` (start it by hand with `sudo systemctl start usbmuxd`).
> 3. Confirm the runtime libraries resolve: `ldconfig -p | grep -E "imobiledevice|libplist|libusbmuxd"`. All three sonames must be present.
> 4. Confirm the phone is visible to the stack itself: `idevice_id -l` (from the `libimobiledevice-utils` package). If that prints nothing, the problem is below AirCard.
> 5. Make sure no other process holds the device — `idevicesyslog`, a file manager, or a mounted `ifuse` share will make the AirTraffic sync time out.

---

## Installation

### Pre-built Executable
1. Download **`aircard.exe`** (Windows) or **`aircard`** (Linux) from [Releases](https://github.com/Lumid-Off/AirCard-Windows/releases).
2. Connect your iPhone via USB, unlock it, and tap **"Trust this Computer"** if prompted.
3. Run the binary. After WiFi sync is enabled, later sessions can work without the cable.

### Linux from a checkout
```bash
sudo apt install usbmuxd libimobiledevice-1.0-6 libplist-2.0-4 libusbmuxd-2.0-7
cargo build --release
./target/release/aircard
```
The first USB connection needs the phone unlocked and the trust prompt accepted. AirCard reads the existing pairing record that `usbmuxd` already holds and never writes a new one, so it will not re-pair or alter a device the user has already trusted.

---

## WiFi Connection Setup
1. Connect the iPhone by USB for the initial pairing.
2. On Windows, in Apple Devices or iTunes, enable **Show this iPhone when on Wi-Fi** / **Sync with this iPhone over Wi-Fi**. On Linux, pair the device once with `idevicepair pair` and make sure `usbmuxd` is started with network device discovery enabled.
3. Apply the setting, then keep the iPhone and PC on the same local network.
4. In AirCard, click **Refresh** and confirm the device shows a **WiFi** transport.
5. Disconnect the cable, click **Refresh** again, and select **WiFi only**. Use **Auto (USB preferred)** when automatic fallback is desired.

If both transports are available, **Auto** uses USB first and falls back to WiFi. For a guaranteed end-to-end WiFi route, disconnect the USB cable, click **Refresh**, and then choose **WiFi only**. This is required because Apple's AirTraffic API selects its route by UDID rather than accepting a transport parameter.

---

## How to Customize Apple Wallet Cards
1. Connect your iPhone through USB or paired WiFi and ensure it is unlocked.
2. In AirCard, stay on the **Wallet** tab and click **Scan**.
3. On your iPhone:
   - Open **Apple Wallet** (or double-click the Side/Power button).
   - Tap the card you want to customize.
   - AirCard intercepts and saves the card hash automatically. Click **Stop**.
4. Click **Choose Image...** to pick your artwork (PNG, JPG, or WebP — automatically center-cropped and scaled to `1536 × 969`).
5. Click **Apply Card Skin**.
6. Force-close the **Wallet** app on your iPhone from the App Switcher (swipe up from bottom, then swipe Wallet away) and reopen Wallet to see your new card!

---

## How to Apply Lockscreen Passcode Themes (.passthm)
1. Switch to the **Passcode** tab in AirCard.
2. Click **Choose .passthm...** and select any `.passthm` package (Cowabunga or Nugget).
3. Select your target iOS version cache:
   - **Auto (TelephonyUI-10)** — iOS 18+ (Default)
   - **TelephonyUI-9** — iOS 16 - 17
   - **TelephonyUI-8** — Legacy iOS
4. Click **Apply Passcode Theme**.
5. Lock your iPhone screen or open Phone dialer to see your new custom passcode keypad buttons!

> [!IMPORTANT]
> **Turn OFF Bold Text:**  
> On your iPhone, go to **Settings ➔ Display & Brightness** and make sure **Bold Text** is turned **OFF**. If Bold Text is enabled, iOS ignores cached dialer button graphics and renders system vector fonts instead.

---

## Building from Source

Prerequisites: [Rust toolchain](https://rustup.rs/), edition 2024 (Rust 1.85 or newer).

| Platform | Toolchain | Extra setup |
| --- | --- | --- |
| Windows 10/11 x64 | `stable-x86_64-pc-windows-msvc` | 64-bit iTunes / Apple Mobile Device Support installed to run, not to build |
| Linux x64 | `stable-x86_64-unknown-linux-gnu` | `pkg-config` and a desktop session for the window |

```bash
# Clone the repository
git clone https://github.com/Lumid-Off/AirCard-Windows.git
cd AirCard-Windows

# Run tests
cargo test

# Build release binary
cargo build --release
```

The compiled binary is `target\release\aircard.exe` on Windows and `target/release/aircard` on Linux. Neither build links against Apple's libraries or libimobiledevice at build time; both load their runtime support stack when the app starts, so the same command works on a machine with no iPhone attached.

The backend is chosen at compile time: `cfg(windows)` selects `src/backend/windows` (Apple's private MobileDevice / AirTrafficHost DLLs through `libloading`), and `cfg(unix)` selects `src/backend/linux` (libimobiledevice, libusbmuxd and libplist, also through `libloading`). Both implement the same traits in `src/backend/mod.rs`, so the device, transfer and UI layers above them are platform-independent.

---

## Contributors
- **[@Lumid-Off](https://github.com/Lumid-Off)** (Windows Native Rust Port & Maintainer) — [GitHub](https://github.com/Lumid-Off) · [Twitter / X](https://x.com/LumidOff)
- **[@mak5er](https://github.com/mak5er)** (Original macOS App & Exploit Research) — [GitHub](https://github.com/mak5er) · [Twitter / X](https://x.com/mak5er)
- **[AirLift](https://github.com/0xjohnnydev/airlift)** by **[0xjohnny (@0xjohnnydev)](https://github.com/0xjohnnydev)**: Original AirTraffic/ATAirlock sandbox escape and proof of concept underlying `AirliftFFI`.

## Credits
- Core exploit based on `airlift` (AirTraffic sync escape).
- Theme format inspired by [Cowabunga](https://github.com/leminlimez/Cowabunga) and [Nugget](https://github.com/leminlimez/Nugget).
