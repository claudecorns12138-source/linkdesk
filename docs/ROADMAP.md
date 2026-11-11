# Roadmap and acceptance

## Milestone 1 — native Mac → Windows extension

- [x] Native receiver and full-screen rendering; no web UI.
- [x] H.264 encoder/decoder transport with generated video.
- [x] TLS, explicit fingerprint confirmation, pairing code, host approval.
- [x] LAN discovery and manual address fallback.
- [x] Implement Mac virtual-display / capture / encode helper and app bundle build.
- [x] Select an existing screen and adjacent edge; preview a direct crossing without moving existing screens.
- [x] Snapshot and check existing display state around virtual-display creation/configuration; abort on changes.
- [ ] Validate the Mac helper on physical hardware and supported macOS versions.
- [ ] Validate Mac native capture → Windows receiver over Ethernet and Wi-Fi.
- [ ] Validate simultaneous original iPad Sidecar + LinkDesk virtual display.
- [ ] Measure frame delivery latency, CPU/GPU, bandwidth, reconnect and suspend/resume.

Acceptance: macOS Displays shows an independent LinkDesk display; three screens can show different Mac windows; the cursor and windows move onto the Windows screen; the session survives a 30-minute document/editing workload and can disconnect/reconnect cleanly. Build success or a synthetic stream alone is insufficient.

## Milestone 2 — daily use

- [x] Chinese native connection flow, retry after failure and in-memory fingerprint confirmation cache.
- [x] Host controls for disconnecting a specific receiver; bounded discovery and control messages.
- [x] On-demand, read-only current-layout and shared-edge checks during Mac sharing; distinguish startup reference from current observations.
- [x] Cancel name resolution / connection / both TLS setup phases, stage-specific retry guidance, and an absolute first-frame deadline that permits static desktops after startup.
- [ ] Saved device identities and revocable device trust, backed by OS credential stores.
- [ ] One-click reconnect and tray/menu bar controls.
- [ ] Receiver resolution/DPI/rotation negotiation and Retina/HiDPI modes.
- [ ] Hardware decoding: Media Foundation/D3D11 on Windows; VideoToolbox/Metal on Mac.
- [ ] Cursor channel, frame pacing, adaptive encoding, bandwidth/latency diagnostics.
- [ ] QUIC transport experiment with a measured comparison against TCP/TLS.
- [ ] Per-device independent virtual displays and automatic session recovery.
- [ ] Signed/notarized release packaging and permission onboarding.

## Milestone 3 — other host / receiver systems

- [ ] Windows IddCx virtual-display adapter, lifecycle, signing, installation and rollback.
- [ ] Native iPadOS receiver using system video decoding; keep existing Sidecar usable.
- [ ] Native Android receiver.
- [ ] Linux capture and compositor-specific virtual display support.
- [ ] Optional receiver input/touch, audio, accessibility and clipboard, each separately enabled.

Default product scope remains local network only. A USB network interface may carry the same protocol; an ordinary cable is not assumed to provide a video input or a network link.
