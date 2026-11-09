# Validation record

Record outcomes, not personal device identifiers. Do not paste raw screenshots of a desktop, pairing codes, fingerprints, hostnames, home directories, private addresses or logs into this public file.

| Check | Current evidence |
| --- | --- |
| Windows native application compilation | `cargo check --all-targets` and strict Clippy passed; distributable release build pending |
| TLS / pairing / native H.264 round trip | All 8 automated tests passed, including actual TLS + approved native H.264 encode/decode |
| Native rendered video | Windows native window visually verified with actual H.264 → TLS → decode → texture playback; frame counter advanced; full-screen entry/exit exercised. Full-screen capture tooling returned a stale image, so full-screen visual delivery needs separate confirmation |
| macOS helper compilation | macOS CI passed compilation, tests and development app/helper bundle build at initial commit `033fb37`; no physical display validation |
| Physical Mac independent extension | Not tested |
| Physical Mac → Windows on wired / Wi-Fi LAN | Not tested |
| Existing Sidecar + LinkDesk concurrently | Not tested |
| iPadOS native receiver / Windows IDD extension | Not implemented |

The automated codec test checks actual H.264 encoding and decoding. The network integration test checks wrong fingerprints, invalid and incorrect pairing codes, explicit owner approval, TLS transport and a decodable keyframe. Protocol tests reject oversized frames, malformed dimensions and incompatible versions. Queue tests prevent unbounded buffers and broken inter-frame continuation.
