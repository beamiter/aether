# Native bounded-core validation (2026-11-13 UTC)

Core pin remains `2b37583ab1a833176e88145e67243146f99c309a`; no production source or dependency lock changes in this checkpoint.

## Executed

- `cargo metadata --locked --filter-platform x86_64-unknown-linux-gnu` completed.
- `cargo build --locked --manifest-path src-tauri/Cargo.toml` succeeded using Rust 1.100.0 and official signed-source Debian trixie GTK3/WebKitGTK4.1 package payloads extracted to a workspace sysroot. No system package installation or maintainer scripts. Debug executable linked successfully; this is not a packaged release.
- Real official Vim 9.1 over a PTY passed: alternate entry, rendered insert, bracketed paste, application arrows, exact Chinese committed text, resize, scroll/edit/undo, save/quit, exact saved content, primary restoration. The test reported 15 unsupported events; this is a bounded smoke, not complete Vim compatibility.
- WebKit subprocess runtime dependency inspection reported no missing libraries in that extracted environment.

Cargo home, target and temporary files were placed on the workspace filesystem instead of the full /tmp tmpfs. Dependency payloads, compiler caches and executable are deliberately not included in this source checkpoint.

## Still not verified

Native window, WebView/Canvas pixels, X11/Wayland input, real platform IME and candidate positioning remain unverified. A fresh AF_UNIX socket creation test returned EPERM both in ordinary and supported escalated execution. A local display cannot be started here. No sandbox or system security setting was disabled.

Prior 102 session/PTY/bridge/Node checks remain in the v12 record. This checkpoint adds native compile and real Vim results, not another run of every prior test.

## Reproduce

Use native-deps/README.md to download/extract official system prerequisites. Set CARGO_HOME, CARGO_TARGET_DIR and TMPDIR to ample workspace-disk directories, then source native-deps/env-native.sh for native build. Keep Rust/host tools out of the extracted sysroot LD_LIBRARY_PATH. For the PTY smoke, run the built session/examples/real_vim under native_run with the extracted official vim.basic path. GUI acceptance requires an environment permitting Unix sockets and normal WebKit sandbox creation.
