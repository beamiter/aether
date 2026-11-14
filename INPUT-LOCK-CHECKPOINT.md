# v16: preserve input locks across asynchronous line writes

2026-11-14 UTC. A shell status / polling failure can arrive while Send line awaits
a write IPC. Previously its successful completion unconditionally re-enabled
input, overriding the lock set by exit or transport/writer failure. A partial
write could also proceed with its remaining suffix after the health lock.

The line writer now checks the lock before every suffix write, and its successful
completion cannot clear an existing lock. No uncertain bytes are automatically
replayed; accepted-prefix reporting remains available. Closing/reopening is
still required after input failure. Output recovery does not unlock input.

Validation: all three new asynchronous lifecycle regressions fail against v15,
and pass with the fix. Full Node suite: 53 / 53, no failures or skips.
Tests use mocked IPC and status delivery, not a native Tauri window, real shell
race, platform IME, pixel test, or physical keyboard acceptance.
Rust, core pin, Cargo lockfiles and Tauri capabilities are unchanged.
Cargo is unavailable in this execution environment; no native build was rerun.
