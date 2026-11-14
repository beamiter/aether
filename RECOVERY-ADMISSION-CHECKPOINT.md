# v17: bounded output recovery admission

Repeated Recover output snapshot clicks previously issued an unbounded number
of full-frame IPCs. Discarding stale responses did not bound the requests.

Now at most one recovery is active per session epoch, including its ACK and
follow-up poll. Additional clicks while busy are ignored, not queued. A failure
allows a new explicit retry; no automatic replay is introduced. A new epoch is
not held up by a stuck old recovery, and the old completion cannot release the
new operation's guard. Input safety locks are preserved.

Node validation: 56 / 56 tests passed. Added burst (200 extra clicks), explicit
retry, and stuck old-session coverage. The burst test fails without the admission
guard and passes after the fix. Tests use simulated IPC rather than a real
Tauri/WebKit window. No native GUI, platform IME, real shell race, or performance
benchmark is claimed. Rust, core pin, Cargo locks and permissions are unchanged.
Cargo is unavailable in this environment. Historical native records are not
a rerun of this version.
