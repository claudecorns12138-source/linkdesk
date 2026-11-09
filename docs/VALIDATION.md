# Validation record

Record outcomes, not personal device identifiers. Do not paste raw screenshots of a desktop, pairing codes, fingerprints, hostnames, home directories, private addresses or logs into this public file.

| Check | Current evidence |
| --- | --- |
| Windows native application compilation | `cargo check --all-targets`, strict Clippy, release build and personal-path binary scan passed; CI development package published. First manual launch of the release package is not yet verified |
| TLS / pairing / native H.264 round trip | All 8 automated tests passed, including actual TLS + approved native H.264 encode/decode |
| Native rendered video | Windows native debug window visually verified with actual H.264 → TLS → decode → texture playback; frame counter advanced; full-screen entry/exit exercised. Windowed playback verified; full-screen image delivery needs separate hardware confirmation |
| macOS helper compilation | macOS CI passed compilation, tests and development app/helper bundle build; no physical display validation |
| Latest code CI | Windows, macOS and Linux all passed format, strict lint, 8 tests, privacy scan and release build at `f89d1ea`: [run](https://github.com/claudecorns12138-source/linkdesk/actions/runs/42554138088) |
| Published development packages | [v0.1.0-alpha.1](https://github.com/claudecorns12138-source/linkdesk/releases/tag/v0.1.0-alpha.1), built from `f89d1ea`; archive text and binary paths inspected before publication; SHA-256 manifest included |
| Physical Mac independent extension | Not tested |
| Physical Mac → Windows on wired / Wi-Fi LAN | Not tested |
| Existing Sidecar + LinkDesk concurrently | Not tested |
| iPadOS native receiver / Windows IDD extension | Not implemented |

The automated codec test checks actual H.264 encoding and decoding. The network integration test checks wrong fingerprints, invalid and incorrect pairing codes, explicit owner approval, TLS transport and a decodable keyframe. Protocol tests reject oversized frames, malformed dimensions and incompatible versions. Queue tests prevent unbounded buffers and broken inter-frame continuation.
