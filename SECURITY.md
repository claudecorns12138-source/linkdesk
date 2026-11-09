# Security and privacy

This is a development prototype for devices on a trusted local network. It is not an internet-facing remote desktop product.

- No account, telemetry, external relay, analytics, persisted identity, saved PIN, or hardcoded credential.
- TLS carries control and video; the receiver pins the certificate after explicit fingerprint comparison. A bare certificate probe sends no application data. Always compare fingerprints before pairing; a code alone does not authenticate a first TLS connection.
- Confirmed fingerprints are cached only in application memory, keyed to the host address. A changed fingerprint prompts again. Reconnecting still requires a valid code and host approval; no device trust is persisted to disk.
- The host requires a valid expiring code and local approval before video leaves it. The connection test uses synthetic video.
- Discovery is unauthenticated and contains only a generic service name/version/port. Treat discovered devices as untrusted until fingerprint confirmation.
- Runtime IPs, pairing codes and fingerprints belong in the local application only. Avoid screenshots of pairing details or private desktops in public issues.
- Private research notes stay outside this repository. Build output, logs, environment files and key/certificate files are ignored. Run the privacy check before every public push.
- The Mac helper uses a private virtual-display interface. It is not a kernel driver or an Apple-supported extension API. Do not ship it as stable without OS and hardware validation.
- Capture/codec threads and socket handshakes are bounded. The protocol rejects oversized records; input from an approved peer is still untrusted. Decoder hardening and fuzzing remain future work.
- Control-record sizes are rejected before allocating/reading their bodies. Discovery queues and displayed host counts are bounded; discovery labels cannot contain control characters.

Please report a suspected vulnerability privately to the repository owner through GitHub's available private reporting channels. Do not include secrets or private captures in a public issue.
