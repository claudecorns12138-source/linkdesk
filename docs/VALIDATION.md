# Validation record

Record outcomes, not personal device identifiers. Do not paste raw screenshots of a desktop, pairing codes, fingerprints, hostnames, home directories, private addresses or logs into this public file.

| Check | Current evidence |
| --- | --- |
| Alpha 3 local checks | Strict Clippy and all 21 Rust tests passed locally. The same C++ immutable-layout guard used by the Mac helper passed its portable native test with warnings treated as errors |
| Alpha 3 native layout UI | Windows native WGPU debug window visually checked with fictional three-screen data: adjacent edge preview, direction picker, occupied-side error, restored valid plan, scrolling and maximized layout. No real Mac display query or mutation occurred in preview mode |
| Windows Alpha 2 local checks | `cargo check --all-targets`, strict Clippy and 15 tests passed; native WGPU debug application exercised on Windows. CI release packages are checked separately below |
| TLS / pairing / native H.264 round trip | All 15 automated tests passed locally, including actual TLS + approved native H.264 encode/decode |
| Native rendered video | Alpha 2 Windows native WGPU debug window visually verified with actual H.264 → TLS → decode → texture playback. Moving generated video filled the screen; F11 / Esc returned to the Chinese windowed interface. Debug synthetic frame rate is not a latency measurement |
| Diagnostic recovery | Disconnect → restart diagnostic restored video. Selecting Share during `--demo` showed an explicit exit action; the diagnostic host remains synthetic, loopback-only and unadvertised |
| macOS helper compilation | macOS CI passed compilation, tests and development app/helper bundle build; no physical display validation |
| Alpha 2 code CI | Windows, macOS and Linux passed format, strict lint, 15 tests, privacy scan and release build at `e11a185`: [run](https://github.com/claudecorns12138-source/linkdesk/actions/runs/42574568193) |
| Published development packages | [v0.1.0-alpha.2](https://github.com/claudecorns12138-source/linkdesk/releases/tag/v0.1.0-alpha.2), built from `e11a185`; Windows x64 PE and both Mac ARM64 Mach-O executables checked. Archive text and personal binary paths inspected; uploaded asset digests matched the local SHA-256 manifest. Downloaded release binaries have not been manually launched |
| Physical Mac independent extension | Not tested |
| Physical Mac → Windows on wired / Wi-Fi LAN | Not tested |
| Existing Sidecar + LinkDesk concurrently | Not tested |
| iPadOS native receiver / Windows IDD extension | Not implemented |

The automated codec test checks actual H.264 encoding and decoding. Network integration tests check wrong fingerprints, invalid and incorrect pairing codes, explicit owner approval, TLS transport, a decodable keyframe, failed-connect completion, host revocation waking a blocked receiver, and changed identity confirmation before sending pairing information. Protocol tests reject oversized control headers before allocation/body reads, oversized video, malformed dimensions and incompatible versions. Queue tests prevent unbounded buffers and broken inter-frame continuation. Discovery and onboarding tests reject misleading labels and malformed input while accepting supported address and PIN formats.
