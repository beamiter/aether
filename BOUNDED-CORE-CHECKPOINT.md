# Bounded core checkpoint

- Date: 2026-11-13 UTC. Source: Library v11, unchanged archive retained before extraction.
- Core/engine exact pin: 2b37583ab1a833176e88145e67243146f99c309a. No shared-core source changes. All three Cargo.lock files changed only their engine Git source revision; dependency versions preserved.
- API: with_bounded_profile(ParserLimits { pending_bytes: 4096, kitty_recovery_bytes: 4096 }, EmberV1). The former with_limits constructor was legacy/unbounded for string retention. This closes that mismatch in the fifth host.
- New file: session/src/bounded_tests.rs. Four tests, matrix over OSC/DCS/APC/SOS/PM, chunk boundaries, exact DCS limit, CAN/SUB, OSC BEL, repeated oversized streams and fresh query recovery.
- Existing CSI-intermediate SGR guard retained, with historical origin clarified. No new keyboard/Kitty claims.

## Executed here

Rust 1.100.0, Linux cloud workspace; Node test runner. See bounded-session-results.txt, bounded-bridge-results.txt, bounded-node-results.txt. Session 51 + PTY integrations 2, bridge 8, Node 41 passed, session and bridge Clippy -D warnings passed. These PTY/Canvas-call tests are not GUI or pixel acceptance.

## Not executed / environment blockers

No new native compile, native WebView/Canvas pixels, X11/Wayland window, real Vim, IME or actual keyboard/clipboard GUI acceptance. Prior checkpoint logs remain historical only. Native Cargo resolution updated its lock, but subsequent dependency unpacking failed because /tmp tmpfs was full; full native metadata/build did not finish. This workspace currently has no restored GTK/WebKit development tree or display stack from the prior environment.

## Recovery and next parallel work

Keep original Library identity; rollback via v11 archive. This checkpoint contains source and logs, excludes targets/caches. No remote prototype repository created or pushed.

Next independent work: restore an isolated native Linux GTK3/WebKitGTK4.1 + Xvfb/Wayland test environment with cache on disk rather than tmpfs; run locked native build and real-input/pixel acceptance. Separately define shared byte-level conformance cases consumed by all five hosts while preserving each host's declared profile. Current prototype is EmberV1-only, limited local host adapter, not complete cross-host equivalence.
