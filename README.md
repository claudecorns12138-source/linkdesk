# LinkDesk

**原生、局域网、跨系统的投屏与扩展屏。目标是像随航一样，把另一台设备变成系统中的显示器。**

Native desktop applications, not a browser viewer or a WebView. Written in Rust, with an Objective-C++ macOS helper. MIT licensed; see third-party notices for dependencies and interface references.

> Development prototype. Native encrypted video transport and desktop rendering are implemented. macOS extension uses an experimental private display interface and requires real Mac validation. This is not a finished Sidecar replacement.

**Development downloads:** [macOS and Windows alpha packages](https://github.com/claudecorns12138-source/linkdesk/releases/tag/v0.1.0-alpha.1). These are native applications built in CI, not browser clients. Follow the pairing instructions below; real Mac extension and Sidecar coexistence remain unverified.

## Current scope

| Capability | Status |
| --- | --- |
| Native Windows / macOS / Linux receiver window | Implemented; platform build/validation results belong in `docs/VALIDATION.md` |
| Native H.264 decoding, full screen, latest-frame rendering | Implemented |
| LAN TLS encryption, fingerprint confirmation, expiring six-digit code, host approval | Implemented; ephemeral identity, no saved pairing yet |
| LAN broadcast discovery; manual host IP fallback | Implemented; routed networks, VPNs and hotspot isolation may block discovery or connectivity |
| macOS independent extended display | Experimental helper: creates one display and captures it with ScreenCaptureKit / VideoToolbox |
| macOS native mirroring | Helper implemented; hardware validation pending |
| Windows primary-display mirroring | DXGI capture + software H.264 fallback implemented |
| Windows independent extended display | **Not implemented**; requires an IDD/IddCx driver and a signing/distribution path |
| iPad / iPhone / Android native clients | **Not implemented**; an existing Apple Sidecar connection can be kept separately |
| One independent virtual display per receiver | **Not implemented**; this prototype sends one source to up to four receivers |
| Receiver keyboard/touch input, audio, clipboard | **Not implemented** |

Wired Ethernet and Wi-Fi use the same IP transport. An ordinary USB-C or HDMI cable is not automatically a network connection or a video input.

## Build and run

Install a current stable Rust toolchain. The lockfile is committed. Building OpenH264 from source needs a C/C++ compiler; NASM is recommended. No external streaming service, account, STUN or TURN server is used.

### Windows

```powershell
git clone https://github.com/claudecorns12138-source/linkdesk.git
cd linkdesk
.\scripts\build-windows.ps1
.\dist\windows\linkdesk.exe
```

MSVC is recommended for distributable Windows builds. GNU development builds copy their toolchain runtime DLLs alongside the application.

### macOS

Install Xcode Command Line Tools and Rust, then:

```sh
git clone https://github.com/claudecorns12138-source/linkdesk.git
cd linkdesk
bash scripts/build-macos.sh
open dist/LinkDesk.app
```

This builds a development `.app` with an ad-hoc signature, not a notarized release. The Mac helper must remain beside the main executable inside the bundle. macOS 14.4+ is the development target, not a guarantee of compatibility with every newer OS.

### Linux receiver

Install the X11/Wayland/OpenGL development packages for your distribution, then `cargo run --locked`. Linux screen capture and virtual-display creation are not implemented. Receiving and synthetic connection tests use the native application.

## Mac → Windows extended desktop

1. Keep the existing iPad Sidecar connection. On Mac, open **LinkDesk.app**, choose **Extend desktop · Mac experimental**, then **Start sharing**.
2. Approve Screen Recording / Local Network access yourself if macOS requests it. If the permission is granted but capture fails, quit and reopen LinkDesk. The application does not modify privacy settings.
3. On Windows, open LinkDesk. Select the discovered host, or enter `<HOST_IP>:47831` manually. Obtain the Mac's address from macOS network settings; do not use the router's IP.
4. On Mac, enable **Show pairing details**, enter its code on Windows, and click **Connect**.
5. Compare the full certificate fingerprint on both devices. Only click **Fingerprint matches · pair** if they match, then click **Allow** on Mac. A probe connection sends no PIN, device name, or video before this confirmation.
6. On Windows, choose **Full screen**. On Mac, check **System Settings → Displays**, arrange **LinkDesk Extended Display**, and drag a Mac window onto it. The Mac's mouse/keyboard operate that desktop directly. Press **Esc** to leave receiver full screen.
7. Confirm that the Mac main screen, existing iPad Sidecar, and LinkDesk screen can show different windows simultaneously. This full combination requires hardware testing; coexistence is not promised by this prototype.

Stopping the Mac host terminates the helper and releases its virtual display. Move valuable work back to the main screen before experimenting with display removal. The first version has fixed maximum 1080p capture, no automatic resolution negotiation, and no saved device trust.

## Connection diagnosis

```sh
linkdesk --demo
```

This runs a native H.264 encoder → TLS loopback → native decoder → desktop rendering path in a single application. It uses generated video, captures no desktop, binds its host to loopback, and does not advertise it on the LAN. It proves local transport/rendering, **not** physical Mac extension, two-device network connectivity, or latency.

For an actual two-device test, choose **Connection test · synthetic video** on the host and pair the other device normally. This exercises real LAN connectivity without sharing private desktop content.

TCP `47831` carries TLS; UDP `47832` carries discovery beacons. Permit the application on the intended local network when your OS asks. Do not disable the whole firewall. Same Wi-Fi name does not guarantee client-to-client connectivity; try manual IP, then a known working Ethernet/LAN path to isolate the cause.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
python scripts/privacy_check.py --self-test
```

The build workflows compile native binaries and the macOS helper. CI is not a real-display acceptance test. See [architecture](docs/ARCHITECTURE.md), [roadmap](docs/ROADMAP.md), [validation](docs/VALIDATION.md), [security/privacy](SECURITY.md), and [third-party notices](THIRD_PARTY_NOTICES.md).
