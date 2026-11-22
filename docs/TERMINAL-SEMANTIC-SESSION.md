# Terminal semantic session

Aether's Terminal tab uses one renderer for local and SSH terminal sessions.
Rust owns the PTY, parses its output and publishes structured cell updates.
The standalone `aether-terminal-host` owns remote PTYs and serves those same
updates across OpenSSH. Agent Workspace attachment remains integration work.

## Identity and attachment

- `open_shell` creates a random 128-bit session ID and returns it as 32
  lowercase hexadecimal digits. The `epoch` advances when the PTY is replaced.
- The `generation` advances when a view detaches or reattaches. Output ACKs,
  polling, history and terminal input require the current session, epoch and
  generation. `close_shell` targets the session and epoch so a close already
  requested for that PTY can finish if recovery rotates the view first.
- A fresh view attaches from a full snapshot and has read-only authorization.
  Recovery from the current view preserves its input authorization and existing
  frontend safety locks. An uncertain input operation is never retransmitted.
- A lost recovery response can be retried using the previous attachment until
  the new snapshot is acknowledged. That retry returns the same generation and
  snapshot. The session ID identifies the PTY on its owning host.

## Snapshots and deltas

`poll_shell` returns an empty response, a `snapshot` with the full v3 cell frame,
or a `delta` with complete replacements for the changed rows and the current
cursor. A delta carries the exact acknowledged `base_revision`. A missing base,
layout or screen change, or a fully changed grid causes a new snapshot. The
WebView validates the identity, base, revisions, row indices, cell values and
cursor before replacing its current state. It acknowledges a frame after applying
and painting it.

The publisher keeps one acknowledged base and one in-flight frame. The coherent
recovery snapshot shares the in-flight frame where applicable. A grid permits up
to 128 rows, 256 columns and 32,768 cells. Each delta replaces at most one copy
of each row. The existing input queue is limited to 16 KiB and output is drained
through the bounded PTY path while a view polls. While detached, the live PTY may
wait for capacity in its kernel buffers until a view reconnects. Closing the
Aether application ends the local owner process and its attachment lifetime.

## Local command line suffix ownership

The desktop form sends the same UTF-8 bytes (including its appended LF) through
local `write_shell`. A zero or short accepted count retains only the exact
unwritten bytes for that view attachment. A later call must supply that exact
suffix; it is not a new command or a request to replay the accepted prefix.
The owner never writes the retained bytes on its own. A replaced/detached
attachment cannot continue them. If a prefix was accepted or the count was
invalid/failed, this raw command route stays interrupted until explicit close.
A successful complete count does not deduplicate a lost full reply; the form
still locks instead of automatically retrying a failed request. This neither
proves that kernel-accepted bytes were executed nor changes the terminal query
reply order during an active raw command. SSH transport has not changed.

## OpenSSH transport

Build the helper on the Linux host with
`cargo build --locked --manifest-path bridge/Cargo.toml --bin aether-terminal-host`.
Run `aether-terminal-host serve` under the account that owns the PTYs. It keeps
the PTYs independently of individual SSH relays. The account's corresponding
relay command is `ssh -T host aether-terminal-host stdio`. OpenSSH supplies the
identity, host-key verification and encrypted transport. The service listens
on `$XDG_RUNTIME_DIR/aether/terminal.sock`, with `/run/user/<uid>` as the
runtime directory fallback. Both helper modes accept an absolute socket path
for a private deployment.

Enter an OpenSSH host alias or `user@host` in **SSH host** on the Terminal tab,
then select **Open SSH shell**. Aether uses the account's existing OpenSSH
configuration, keys and verified host key. **Recover output snapshot** opens
a new relay when needed and paints the recovered snapshot before sending its
ACK. Input resumes after this ACK if the session was writable and the old
input state was certain. **Detach view** closes the relay. **Reconnect view**
opens a read-only attachment, and **Close shell** closes the identified PTY.
Closing Aether closes its relay. The remote PTY remains with the owner until
explicit close or the inactivity timeout.

An application launcher can set `AETHER_SSH_TERMINAL_HELPER` and
`AETHER_SSH_TERMINAL_SOCKET` to absolute paths on the SSH host for a private
deployment. Aether quotes these settings as separate remote arguments.

The relay sends one newline-terminated UTF-8 JSON request at a time. The owner
accepts protocol version `1` and a nonzero `request` number. It returns the
same request number and a `result` object with either `Ok` or `Err`.
For example, a client can open a 24×80 PTY with this request.

```json
{"protocol":1,"request":1,"command":{"kind":"open","rows":24,"cols":80}}
```

The open reply supplies the first attachment. Use `poll` and `ack` with that
attachment to apply the existing v3 snapshot and delta records. After a lost
connection, `recover` on a new SSH relay takes the old attachment and returns
the current full snapshot and a new generation. The same recovery can be
retried until its first snapshot ACK. Once the generation advances, the owner
refuses input, ACKs and close requests from the old generation. Explicit
`detach` followed by `attach_read_only` takes the stable session ID and epoch
and opens a read-only view. Terminal input and creation requests go to the
owner once per request. The caller treats an interrupted input or open as
uncertain and can recover an already identified session.

The owner limits an account's service to four live PTYs and expires a session
after ten minutes without a successful request. Each request is bounded to
128 KiB and each reply to 16 MiB. The existing grid and input limits apply.
The socket directory and socket are private to the account. A lock serializes
owner startup, and a live endpoint keeps its socket during a competing start.

The cross-machine test starts a private owner on the designated host and
checks grid reconstruction, two lost SSH connections, stale generation
rejection, read-only attachment and PTY cleanup.

```sh
python3 scripts/test-ssh-terminal-session.py host /absolute/path/aether-terminal-host
```

The native Xvfb/WebKitGTK check drives the actual Terminal tab, kills its
owned OpenSSH relay, recovers the displayed grid, submits fresh input and
checks read-only reattachment and remote cleanup.

```sh
python3 scripts/test-ssh-terminal-webview.py /absolute/path/tauri-terminal-lab host /absolute/path/aether-terminal-host
```
