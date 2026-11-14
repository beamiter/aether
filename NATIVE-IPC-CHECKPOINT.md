# v19 — native Tauri command dispatch (2026-11-14 UTC)

This checkpoint restores native Rust validation and changes the actual Tauri host,
not the JavaScript terminal model. It is still an experimental fifth terminal.

## Change and regression evidence

All 14 custom commands previously locked and executed the backend synchronously
inside the Tauri invoke handler. A contended backend lock, PTY spawn or parsing
work could therefore hold the window event thread. Commands now use async Tauri
handlers and a common `spawn_blocking` adapter with `Arc<Mutex<...>>` managed state.
The mutex is acquired inside the blocking task; no guard crosses await. Worker
join failures and poisoned state become IPC errors. Existing command names,
arguments, wire output, core pin and permissions are unchanged. Concurrent calls
remain serialized by the backend mutex; this is not a global FIFO guarantee or
an IPC flood/resource-budget implementation. Frontend admission guards remain
necessary. App exit/direct-child cleanup limitations still apply.

`application(builder)` shares the exact production registrations with two Rust
integration tests. The tests use Tauri's mock **window runtime**, real command
macros, JSON decoding/response serialization and real Linux PTYs. They are not
WebKit IPC transport, real-window, rendering or IME acceptance.

- Dispatch regression: keep backend state locked on a helper thread; invoke
  `shell_status` through Tauri and require dispatch to return before the lock is
  released. Replayed the v18 handlers with only the builder extraction and test
  module attached: it failed with `dispatch blocked: 3.004212458s`. Restored v19:
  passed. See `v19-before-dispatch.txt` and `v19-native-tests-final.txt`.
- Real PTY through the registered Tauri handlers: open, reject malformed epoch
  and stale write, resize, enqueue logical text/Enter, poll/ACK until exit and
  drain, recover and verify an output-only sentinel and geometry, history,
  close, reject closed epoch, reopen with a new epoch. Passed.

The Tauri manifest enables the `test` feature only as a dev dependency and uses
serde_json for test assertions. The native lockfile adds only the direct
serde_json dependency edge; package versions did not change. Session and bridge
lockfiles are byte-identical to v18. All three retain core
`2b37583ab1a833176e88145e67243146f99c309a`. No xterm.js or JS ANSI parser was added.
No remote repository was created and no prior Codex thread was read. The supplied
archive was materialized into this execution environment via the Library skill
and was readable before extraction. No checkout `.agents/skills` directory or
applicable SKILL.md was present.

## Executed on this checkpoint

Rust 1.100.0, Debian 13, workspace-only rustup and official signed-source Debian
package payloads. No system package installation or maintainer scripts.

| Check | Result | Log |
| --- | --- | --- |
| session `cargo test --locked` | 57 passed (51 unit + 2 PTY + 4 contract) | v19-session-tests.txt |
| bridge `cargo test --locked` | 8 passed | v19-bridge-tests.txt |
| native `cargo test --locked` | 2 passed | v19-native-tests-final.txt |
| Node `npm test` | 60 passed | v19-node-tests.txt |
| session/bridge/native Clippy `--locked --all-targets -- -D warnings` | all passed | v19-*-clippy.txt |
| native `cargo fmt --check` | passed | v19-native-fmt.txt |
| native `cargo build --locked` | Linux debug executable linked | v19-native-build-final.txt |
| actual official Vim over PTY | passed; 15 unsupported events retained | v19-real-vim.txt |
| WebKit subprocess `ldd` under extracted runtime | no missing libraries reported | v19-webkit-ldd.txt |

127 tests passed, plus the real Vim smoke. The Vim smoke covers alternate screen,
insert, bracketed paste, application arrows, Chinese committed text, resize,
scroll/edit/undo, exact save/quit and primary restoration. It does not exercise
a platform input method or prove complete Vim compatibility.

## Actual native launch boundary

Unlike the earlier environment, AF_UNIX creation succeeded. Authenticated Xvfb
`:99` started with TCP disabled; xdotool reported `1280 1200`. This only validates
a display server. No terminal window or pixels were accepted.

The first application launch failed creating application state on a read-only
filesystem. Standard XDG config/cache/data/runtime directories were moved into
private workspace directories for the next launch. It then failed (exit 133):
`Unable to spawn a new child process ... /usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/WebKitNetworkProcess (No such file or directory)`.
The executable exists in the extracted sysroot, but this release WebKit uses its
compiled absolute subprocess directory. `WEBKIT_EXEC_PATH` is developer-mode-only
in upstream source and did not redirect this package. The current user is uid
1000 and has no sudo. We did not place files in system directories, patch WebKit,
mount over paths, change permissions, or disable its sandbox.

See `v19-native-launch-readonly.txt`, `v19-native-launch.txt` and
`v19-display.txt`. GUI, pixels, actual keyboard/clipboard/composition, candidate
positioning, Wayland and native responsiveness remain **unverified**. The mock
runtime dispatch test establishes scheduling behavior, not a measured GUI frame
rate. No daily-use claim is made.

Initial native compilation needed extra development/runtime payloads because
apt's download-only dependency resolution skips packages already installed on the
host. Missing pkg-config dependencies and dangling sysroot runtime symlinks caused
intermediate failures; extracting the matching official packages resolved them.
The final success logs above are authoritative. The runtime runner keeps extracted
libraries out of Cargo/rustc's environment. See native-deps/README.md and the package
inventory; binary/dependency caches, display credentials and screenshots are not
included in this checkpoint.

## References used to restore prerequisites / diagnose launch

- https://rust-lang.org/install — official rustup installation.
- https://v2.tauri.app/start/prerequisites/ — official Linux dependencies.
- https://github.com/WebKit/WebKit/blob/main/Source/WebKit/Shared/glib/ProcessExecutablePathGLib.cpp — developer-mode guard around WEBKIT_EXEC_PATH and compiled fallback.
