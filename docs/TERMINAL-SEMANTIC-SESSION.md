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
- An explicit reconnect attaches from a full snapshot and has read-only authorization.
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

The local desktop requests frame version 5 through `subscribe_local_shell`.
The registry's worker advances the PTY and sends frame and shell status updates
through a Tauri Channel. Each new stream starts with a full snapshot. Versions 4 and 5 carry the screen, wide-cell spans, full underline type and
color, and physical row wrap flags. The visible copy action uses the flags to
keep a soft-wrapped line together. The local Channel preserves bounded
Unicode clusters across PTY reads, including combining marks and wide joined
emoji. Cell spans currently follow the first graphic scalar in the cluster.

Local OSC 8 links use the same validated absolute HTTP(S) target policy as
Ember. Each Channel update carries the targets named by its rows. The WebView keeps
targets only while decoded rows reference them and rejects a changed target for
an existing ID within the same session incarnation. Ctrl-click resolves the
cell in the exact acknowledged frame under the current owner before asking
the desktop's default URI handler to open it. Hyperlink targets and terminal
cell text have independent byte budgets.

Version 5 also carries immutable image resources and per-row image slices. A
full snapshot includes every visible resource. A delta can reference a resource
in the exact acknowledged base. The WebView rejects missing resources and drops
a resource when its last visible slice leaves the current grid. Image IDs use
canonical decimal strings and share the session incarnation with hyperlinks.
The engine admits up to 32 raster resources and 2 MiB of RGBA per local PTY,
including resources referenced by retained history or immutable frames. The
local canvas displays static direct Kitty RGB, RGBA, and PNG transfers on the
primary screen.

The local history request uses the same Channel cell and raster types in a
read-only page. Each paint row carries its original archive anchor and the
UTF-16 copy boundaries of its cells. The history canvas paints the visible
rows plus a small margin. The canvas and copy field select the same text.
Ordinary history pages reflow at the current terminal width. A search result
keeps the original geometry of its immutable search window and selects the
exact match, including a bounded prefix across the next window. The stream
reports the oldest retained source row so clearing history or evicting a page
invalidates its Canvas and selection. SSH retains the text history response.

The WebView checks the 8 MiB encoded frame budget with a separate allowance for
canonical base64 image data. It validates the cluster, total text, grid and
raster budgets before applying the update. Frame revision, layout,
publication sequence and ACK use canonical positive u64 decimal strings. The
WebView compares them at full precision and validates the attachment, sequence,
frame and status. It applies and paints the frame at the next animation callback
before sending `ack_local_stream` with the channel ID, sequence and frame
revision, when present. An idle healthy session sends a status heartbeat. A slow
view holds one envelope in flight.
The worker keeps parsing and draining the PTY, then publishes the latest grid
against the acknowledged base. Local tab activation, attach and recovery can
negotiate Channel full snapshots and acknowledge their decimal revision before
opening the stream. A rejected paint frame recovers a full Channel snapshot
under a new attachment generation. A stalled transport can recover a v3
snapshot and continue polling if its counters fit. SSH retains its polling and
wire contracts.

The publisher keeps one acknowledged base and one in-flight frame. The coherent
recovery snapshot shares the in-flight frame where applicable. A grid permits up
to 128 rows, 256 columns and 32,768 cells. Each delta replaces at most one copy
of each row. The existing input queue is limited to 16 KiB. A detached local desktop PTY
can also drain under the same owner if there is no reader and no outstanding
raw or logical write boundary. Each turn reads at most 64 KiB and may send only
the parser's bounded protocol replies. It does not publish, acknowledge, or
resume input. If the writer is interrupted or its input boundary is uncertain,
the detached owner stops polling rather than sending more bytes; the PTY may
wait for kernel buffer capacity until a view reconnects or it is explicitly
closed. Closing Aether ends the local owner's attachment lifetime. SSH and
standalone helper retain their v3 request and frame contracts.

## Local history search

`search_local_history` uses the current attachment to search the shared engine's
primary-screen archive. It accepts a literal query of up to 128 UTF-8 bytes and
checks at most 64 source rows per step. Matching text uses the engine's copy
projection, including soft wraps and certified wide-character margin gaps. A
matching window returns its immutable text and UTF-16 selection range. A miss
advances the search without transferring the page to the WebView.

The search begins with the newest retained output. A search ID orders requests
within the current attachment, and the first result fixes an engine revision.
If output or geometry changes during navigation, start a new search. The
current selection remains valid in the displayed snapshot. Cancel stops the
next step, and a replacement attachment discards the old search.

## Raw input suffix ownership

The desktop form sends the same UTF-8 bytes (including its appended LF) through
local `write_shell`. The SSH host uses the same boundary for its raw write
request. A zero or short count retains the exact unwritten bytes for that view
attachment. Each retry must send that suffix explicitly. The runtime holds
terminal replies between an accepted prefix and its final byte. A successful
count records bytes accepted by the kernel. The form locks after an uncertain
receipt to prevent replay. Retiring a started prefix revokes input until close
and allows the PTY's output to keep draining.

## Two local shells (desktop only)

**New shell tab** adds a second PTY without a separate detach step. Selecting
an inactive tab uses `activate_local_tab`, which moves the reader under the
local registry mutex and publishes a full snapshot for a fresh generation.
The outgoing input lease is retired. A tab parked with confirmed, idle input
and healthy raw and logical writers may reclaim input after acknowledging its
snapshot. A locked, interrupted, or explicitly detached view cannot gain input
through this path. Only one tab owns input and resize authority at a time.

If the activation reply is lost, the same request retrieves the same pending
snapshot until ACK. If the ACK reply is lost, the WebView retries that exact
ACK. A tab switch never resends shell input from the previous view or starts a
duplicate PTY. After closing one tab, the remaining tab can be selected with
its parked grant. Tab processes and drafts live only in the current desktop
process.

**Split panes** places two local shells side by side in stable tab order. The
inactive shell contributes a read-only snapshot to the active Channel delivery.
The envelope uses the same single delivery credit for both panes. Compatibility
polling reads the inactive snapshot separately. Clicking the inactive pane
selects it through the same tab-activation and input lease path. Only the
selected shell accepts input and manual geometry changes.

The WebView profile stores a versioned shell layout with the tab count,
selected position, and split choice. Version one records open in single-pane
tab mode. After a desktop restart, **Restore shell tabs** creates fresh local
PTYs following the saved layout. An unconfirmed launch is discovered through
the live roster. If the second launch fails, the first remains available and
the saved layout survives for a later restart.

With the current view detached, select **Show retained local shells**. If
exactly one local shell exists, **Open second local shell** explicitly starts
one more writable shell without closing the first. Choose a retained local
number through the list for explicit view-only reconnect. Reconnecting
view-only does not grant resize or input authority;
ordinary Open won't start a new shell while any local shell remains. Use
**Close shell** on the view you intend to close. If an open response is lost,
list and choose the exact local shell; the app does not automatically open
another.

The list returns only owner-local epoch numbers, not attachment identifiers,
generations, working directories or history. It does not allocate a shell or
perform IPC against SSH. While a different view is attached, each inactive
write-idle PTY can still advance under the same owner. The two-entry scheduler
uses round-robin choice and at most one bounded detached poll per turn.

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
explicit close or the inactivity timeout. While no relay is attached, the
owner continues bounded PTY reads and ordered protocol replies, retaining the
latest complete paint boundary for recovery. Status and paint ACKs do not
gate this background progress.

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
after ten minutes without a successful request or PTY output. Each request is
bounded to 128 KiB and each reply to 16 MiB. The existing grid and input limits apply.
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
