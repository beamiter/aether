# Aether

Aether is an experimental Linux terminal built with Tauri 2 and the shared Rust
`jterm_engine`. Rust owns ANSI parsing, screen state and input encoding; the
WebView renders validated frames on Canvas. There is no xterm.js or JavaScript
ANSI parser. **This is not a daily-use terminal.**

## Shared core

The three Cargo manifests pin the shared crates to
`9d9d4fd28e1e60fc4c22f996c7d7b3f709d71bff`. `jterm_runtime` owns the local PTY, direct child and
output drain. Its `jterm_engine::terminal::Terminal` owns parsing, screen
state, input encoding, history and immutable frames. The session host supplies
each frame stream's epoch. Run `python3 scripts/check-core-pin.py` to verify
that every shared package in the manifests and lockfiles uses that published commit.
[Core upgrade verification](docs/CORE-UPGRADE.md)

## Terminal session synchronization

Open the experimental shell to start a semantic session. **Recover output
snapshot** resynchronizes the current view. **Detach view** leaves the shell open
for **Reconnect view**, which restores the output in read-only mode. Reloading
the desktop page also leaves the local shell running. **Reconnect view** restores
its output, including output produced during the reload.

Use **New shell tab** or **New jsh tab** to open a second local shell. Select the
numbered tabs to return to each shell's output, draft, and working process. A tab that was parked
with idle, healthy input can write again after its current snapshot is
acknowledged. Switching does not submit drafts or repeat pending input. Closing
the active shell leaves any surviving tab available for selection. **Detach
view** and **Reconnect view** still provide an explicit read-only path.

After restarting Aether, **Restore shell tabs** opens fresh PTYs with each
saved shell type and Terminal Chat preference. It restores the selected tab
and the split layout. With two shells, **Split panes** shows both outputs side
by side. Click the other pane to transfer keyboard input and sizing to that
shell. Closing the last shell clears the saved layout.

For a remote PTY, run `aether-terminal-host serve` on the Linux host and use
**Open SSH shell** with an OpenSSH host alias or `user@host`. The same terminal
view displays local and SSH cell updates. After an SSH interruption, **Recover
output snapshot** can reconnect to the current shell.
[Session identity and wire contract](docs/TERMINAL-SEMANTIC-SESSION.md)

Rust reuses the preceding immutable capture when the revision is unchanged
and shares the cell storage of unchanged rows across stable layouts. Canvas
updates changed rows and the rows affected by cursor movement when row edges
align to backing pixels. Snapshots, geometry changes, and fractional row edges
use a full repaint.

The local desktop receives frames and shell status through a Tauri Channel.
Rust advances the PTY independently and keeps one delivery in flight until
the view acknowledges it. The live canvas displays bounded static Kitty RGB,
RGBA, and PNG images sent directly by the local PTY. Images move with their
rows and reconnect with the retained shell. **Recover output snapshot** can
replace an invalid Channel frame or resume compatibility polling after a
transport stall. SSH terminal sessions use polling over the existing relay.

Use **Refresh history / copy snapshot** to read the local archive. The history
viewport displays retained styles and images at the current terminal width.
Select cells in the viewport or use the text field to copy its snapshot.
**Find in local history** searches the selected shell with a literal,
case-sensitive query and selects the match. **Older match** continues the
same search, and **Cancel** stops between history windows.

With jsh installed on the application's PATH, **Open jsh with Blocks** starts
an integrated local shell. **Command blocks** shows completed commands with
their reported exit status, directory, source, and bounded plain-text output.
Expand a card to copy its output. **Insert command at shell prompt** inserts
the selected retained revision into the active jsh line for editing. Press
Enter to execute it. A foreground program or a retired record cannot receive
the insertion. **Back to live terminal** returns keyboard
focus to the same PTY. Command blocks are kept in memory for that shell.
[Shell integration contract](SHELL-INTEGRATION-CONTRACT.md)

**Find in completed commands** searches the retained commands and their
captured output by literal text. Choose a result to open the matching record
and select the text in its original command or output. Expand a card to
**Bookmark** it. **Bookmarked only** shows saved cards from that shell in
both the command panel and Terminal Chat. A bookmark follows its live PTY
until its command leaves the bounded history or the shell closes.

**Terminal Chat** in the local tab bar places completed command cards above
that tab's live shell. At a ready jsh prompt, **Send command** presses Enter in
the existing shell editor. Foreground programs keep receiving keyboard input
through the same terminal. Fullscreen programs occupy the pane, and ordinary
shells retain the live terminal. The selected presentation follows the local
tab while its PTY remains open.

At a writable jsh prompt, open **Workflows** or press **Ctrl+Shift+M** in the
terminal. Aether loads TOML and YAML from the shared workflow format. It reads
the user configuration directory's `aether/workflows` first, then directories
in `AETHER_WORKFLOW_DIR`, followed by the user and system data directories'
`aether/workflows`. The first workflow with a given name wins. Search by name,
description, or tag, choose an entry, and fill in its arguments. **Insert at
cursor** puts the rendered line into the original jsh editor. Review the line
there and press Enter to run it.

### Eva Companion

Launch Aether with `EVA_COMPANION_MODE=shared` in the graphical session that
runs `jcompaniond` with host feature negotiation. The **Eva** control shows
or hides its native slot in this window. Eva follows the selected local shell.
When jsh reports a finished
command, the slot shows its success or failure and the daemon receives the
outcome with an opaque command ID. The feedback clears when the active tab
changes or enters an alternate-screen program.

Eva requires `wp_presentation` on Wayland or extended `_NET_WM_FRAME_TIMINGS`
from a compositing window manager such as JWM on X11. Aether waits for
presentation feedback before requesting a shared body. Closing a window that
owns one waits for the cleared frame's feedback and the daemon's acceptance.

## SSH files and read-only Workspace

In **SSH Files**, enter a known OpenSSH host and an absolute Linux project
directory. Install `aether-terminal-host` on that host's PATH. Aether starts
`aether-terminal-host files` on an independent SSH connection and holds the
chosen directory open until **Close SSH project**. Browse directories, preview
UTF-8 text up to 256 KiB and transfer files up to 1 MiB. Uploads create new
files and preserve existing names. The browser skips symlinks. A preview labels
the host, path and SHA-256. Downloading a previewed file requires the same
bytes. Preview again to select a changed version.

In **Remote Workspace (read-only)**, enter the SSH alias, the absolute path to the
remote Workspace-capable executable, and its expected host ID. Obtain the ID
on that host with `tauri-terminal-lab --jterm-workspace-host-id` when using
Aether as the executable. Browse catalog pages, search all conversations or
one selected conversation, and open an exact evidence revision. If the host
replaces a result, run the search again to select the new revision.
[Native SSH acceptance](docs/NATIVE-LOCAL-WEBKIT-SMOKE.md#ssh-files-and-remote-workspace)

## Agent Workspace

Open **Agent Workspace** to list the CLI conversations in the shared local
session host. The catalog identifies each provider history by its original
profile and session ID. Select a conversation and choose **Attach terminal**
to resume it or take over its running terminal after the other view detaches.
**Detach terminal** releases the input lease while the host keeps the provider
running. Reopening Aether can attach to that same provider later.
[Native Workspace validation](docs/NATIVE-LOCAL-WEBKIT-SMOKE.md)

**API conversations** uses the same Workspace host for saved multi-line
drafts, streaming replies and recent conversation history. **Save draft**
commits the text to the shared conversation. **Send message** submits the
acknowledged draft and the host continues the request while Aether is closed.
Reopen the conversation to see its stream or completed reply. **Cancel
request** is available to the view holding the request's control lease.

Set `AETHER_AI_PROVIDER` in Aether's native launch environment to enable
submission. Rust uses the shared family loader for `AETHER_AI_MODEL`,
`AETHER_AI_BASE_URL` and any configured provider credential or
`AETHER_AI_API_KEY_FILE`. Drafts and existing conversations remain available
when provider settings are absent.

**Native tasks** starts a Codex task from an explicit goal and a committed
local Git repository. The shared worktree service creates an isolated
checkout, and the Workspace host owns the provider runtime. Aether displays
the provider's output and pending review evidence.
The current native approval policy allows the controlling view to decline a
request. Closing the view lets a later attachment recover the same pending
request. **Finish native session** releases an idle provider, and **Inspect
result** reads the worktree diff and validation status from the shared host.
Add a verification command when authoring a task to run that exact one-line
command in the worktree with `/bin/sh`. The command is included in the task brief
shared with Codex. **Run verification** starts a separate
host-owned execution after the provider stops. Its output is available after
reopening Aether. **Accept verified work** becomes available after inspecting
the passing result. A worktree change invalidates the evidence and requires
another verification run before acceptance.
For an authored task without a verification command, inspect the finished
worktree and choose **Accept reviewed work**. The host checks that the worktree
still matches the inspected version. If it changed, inspect the updated diff
before accepting.

Open **Task context** on a selected native task to save its handoff goal,
completed work, remaining work, constraints or a note in the shared Workspace.
**Preview Codex handoff** assembles the current saved revision. **Open new
task draft** uses that preview to populate the existing task form. Review
the draft, confirm sharing, and start it to create a separate task and
worktree. If another window changes the context, reload to see its saved
revision and preview again.

**Missions** creates a delivery plan from the selected native task's exact
worktree, baseline and run. Give the plan an acceptance goal and repository
target, then inspect local Git and validation evidence through the shared
Workspace host. A current clean inspection can freeze the observed candidate.
Saved inspections reopen as historical observations. Inspect again to verify
the current candidate and its delivery gates.

Use **Tools** to search the shared evidence index and open a particular saved
record and revision. A search can cover the workspace or the selected native
task. The index includes completed validation output and reports its current
coverage. If a hit changed since search, run the query again for its current
revision.

**Save task draft** records the exact authored task and managed worktree
before provider launch. Open **Task queue** to save its dependency plan.
The host checks that dependencies have been accepted and the saved
worktree still matches. A ready draft starts only after you confirm sharing
and choose **Start queued task**. The queue and draft survive reopening.

## JWM remote desktop

With `jwm-remote` installed on PATH, open **Remote Desktop** and enter the
host address. Connections start in view-only mode. The X11 viewer accepts
keyboard and pointer control when enabled explicitly and granted by the host.
The Wayland viewer supports an explicitly granted one-shot keyboard and
pointer lease per authorized window. Releasing it keeps the viewer open in
read-only mode. Use **Source** to switch among the host's authorized sources.
In a managed Wayland session with several authorized windows, a switch releases
the current lease first. Once the new window has presented, **Request Input**
can activate its unused grant. **Release Input** releases the X11 grab or the
Wayland lease. **Disconnect** closes the viewer.

Select **Modifier clicks and dragging** with Wayland control for host-approved
compound gestures. Aether shows when a presented frame is ready to start a drag.

When Aether runs on X11 and **Viewer** is set to **X11**, its native video pane
uses the GTK XEmbed socket. Use a `jwm-remote` build that has
`session --x11-parent-window`. The **Terminal** tab hides the pane and releases
its input grab while keeping the connection open. Other display and viewer
combinations use a separate native viewer window. The WebView receives control
state and performance metrics. Closing Aether ends the connection. Recent
hosts and results are held for the current window session.
[GtkSocket reference](https://docs.gtk.org/gtk3/class.Socket.html)

To check the X11 pane with private displays, run
`python3 scripts/test-remote-desktop-x11.py PATH_TO_AETHER PATH_TO_JWM_REMOTE`.
The check uses Xvfb, Metacity, xdotool, xwininfo, xprop, xsetroot, wmctrl,
ImageMagick and Python Pillow.

## Build and check

Use Linux, Python 3.11+, Node 24 and the Rust toolchain specified in
[rust-toolchain.toml](rust-toolchain.toml). Install the official distribution
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). The CI workflow
uses Ubuntu 24.04 packages for GTK3, WebKitGTK 4.1, librsvg, libxdo, OpenSSL and Vim.
It never disables the WebKit sandbox.

```sh
python3 scripts/check-core-pin.py
npm ci --ignore-scripts
npm test
for crate in session bridge src-tauri; do
  cargo fmt --manifest-path "$crate/Cargo.toml" --check
  cargo test --locked --manifest-path "$crate/Cargo.toml"
  cargo clippy --locked --manifest-path "$crate/Cargo.toml" --all-targets -- -D warnings
done
cargo run --locked --manifest-path session/Cargo.toml --example real_vim -- /usr/bin/vim
cargo build --locked --manifest-path src-tauri/Cargo.toml
# On a desktop with normally installed WebKit subprocesses and a working display:
npm run tauri -- dev
```

There are three independent Cargo manifests, not a root Cargo workspace.
`src-tauri` depends on `../bridge`, bridge on `../session`, and Tauri embeds
`../web`. Internal crate/package identifiers remain those of the imported lab;
they are not additional terminal engines.

### Debian candidate package

On a Debian or Ubuntu build host with the same prerequisites, build the desktop
and SSH host from the same checkout. The package installs `tauri-terminal-lab`,
`aether-terminal-host` and an Aether Terminal desktop entry.

```sh
npm ci --ignore-scripts
export CARGO_TARGET_DIR="$PWD/target"
cargo build --release --locked --manifest-path src-tauri/Cargo.toml --bin tauri-terminal-lab
cargo build --release --locked --manifest-path bridge/Cargo.toml --bin aether-terminal-host
scripts/package-linux-deb.sh "$CARGO_TARGET_DIR/release/tauri-terminal-lab" \
  "$CARGO_TARGET_DIR/release/aether-terminal-host" ./target/packages
```

To rehearse an upgrade, package two distinct builds. Pass a higher package
version as the fourth argument for the candidate. With `fakeroot` installed,
run `python3 -B scripts/test-linux-deb.py OLD.deb CANDIDATE.deb`. It uses a
private package root and checks that Workspace can reopen the pre-upgrade backup
with the previous binary. Keep the previous package and a pre-upgrade backup
together when preparing a rollback.

## What validation covers

[Linux CI](https://github.com/beamiter/aether/actions/workflows/ci.yml) runs on
pushes to master, pull requests and manual dispatch. Check the run for the exact
commit; this link alone is not a claim that a particular commit passed.

- Node tests use mocked DOM/Canvas calls and IPC to exercise frame validation,
  input safety locks, paste review, history and recovery admission.
- Rust session/bridge tests cover parsing, bounded state, real Linux PTYs,
  epochs, frame ACK/recovery and direct-child lifecycle behavior.
- Tauri tests use the actual command registrations and a **mock window runtime**
  connected to real PTYs. They do not test the WebKit IPC transport.
- The Vim smoke drives actual Vim through a PTY and checks screen transitions,
  edits, resize, exact saved content and primary-screen restoration. Chinese
  committed text is tested; a platform IME is not.
- The Linux build job links a debug desktop executable without launching it.
- The separate [native WebKitGTK smoke workflow](https://github.com/beamiter/aether/actions/workflows/native-webkit.yml)
  launches the built Aether app under X11/Xvfb and drives it with WebDriver.
  The [run at f5ccee2](https://github.com/beamiter/aether/actions/runs/44991481517/job/137113278722)
  verified two real local shells, distinct output and Canvas colors, explicit
  read-only reconnection, isolated drafts and reaping of both direct children.
  [Smoke setup and scope](docs/NATIVE-LOCAL-WEBKIT-SMOKE.md)

This native smoke does not test physical mouse input, compositor screenshots,
Wayland, platform IME or native clipboard behavior. Process ownership covers
only the direct shell, not descendant containment. The product shell uses
`TERM=dumb`; the Vim smoke opts into `TERM=xterm`. Unsupported sequences and
Unicode shaping limitations remain explicit. See [lifecycle policy](LIFECYCLE-POLICY.md) and
[session contracts](session/README.md).

## History

Aether was imported from the v19 terminal laboratory checkpoint. Historical counts,
environment failures and archive-only log references are retained in
[lab history](docs/LAB-HISTORY.md), [validation history](VALIDATION.md), and
[import notes](PUBLISHING.md). They do not override the current pin, build commands,
CI result or acceptance limits above. The old Library archive is not the maintained
source; this repository is.

## Local notebooks

In **Notebook**, choose a UTF-8 `.jtnb.md` file up to 1 MiB, then click
**Open notebook**. Aether shows the chosen source snapshot and its SHA-256.
Opening, closing, and reopening the file never run its cells. Reopen the file
to read changes on disk.

Each supported shell fence has **Run cell** and **Stop cell** controls.
Unlabelled and `shell` fences use `/bin/sh`. The panel shows the latest run,
including stdout and stderr separately, a combined 256 KiB output limit, and
the process exit or stop status. Stopping preserves collected output and
terminates the cell's owned process group. Other fences stay readable.

Cells run as the local user in a new temporary working directory for each
opened notebook. Relative paths refer to that directory. The file picker
supplies only the selected bytes and basename.
[Native Notebook check](docs/NATIVE-LOCAL-WEBKIT-SMOKE.md)

## Frozen command output

In a local **jsh with Blocks** session, open **Command blocks**, select two
completed commands under **Before** and **After**, and click **Compare
captured outputs**. The comparison retains both record IDs, content revisions,
and the attachment that supplied them. Later terminal output and command
retention changes leave this review intact.

Use **Export Markdown** or **Export JSON** to download the frozen review.
Each export includes both captured records and the comparison. Commands whose
output used live-screen operations or exceeded the capture limit remain in
the block list without a comparison option.

## Vim9 Workbench

Choose **Open local file in Vim9** and select a file in the native Linux
chooser. The workbench uses a dedicated Vim 9 PTY and paints Vim's authenticated
buffer frames over the editor area. The terminal supplies Vim's prompt and
status rows. **Show live terminal** switches to that same Vim session, and
unsupported or disconnected buffer rendering switches there automatically.

Type in the editor, then choose **Save in Vim** to write through Vim to the
chosen file. **Quit Vim** asks Vim to close normally, so modified buffers
still use Vim's own save rules. **Close workbench** releases the view after the
PTY exits. **Project tools** lets you browse folders, jump to
`path:line:byte-column`, or type `>command` to use Vim's palette. Search
fills Vim's quickfix list. A build shows its output and exit status and can
be stopped while running. The Canvas paints Vim's completion menu and
supported plugin popups in Vim's stacking order. Vim handles selection and
dismissal. Unsupported frames switch to the live terminal.

## Experimental local source preview

Choose a trusted local directory in the page to authorize a browser `File` snapshot
(up to 2048 files total, checked before iterating the FileList). For larger projects,
use **Select one source file** to select the desired file directly without
enumerating its directory. Single-file mode supports manual preview only: its
basename does not establish a project-relative path and is not matched by
terminal Ctrl-click. Selection replaces previous access; Clear removes both modes.
The folder limit remains unchanged, including unsupported file types. Select a
smaller subtree if terminal-relative matching is needed. Ctrl+left-click an exact visible relative name such as
`src/App.tsx` or `README.md` to select it in the sidebar, then confirm **Preview
selected local file**. This confirmation is intentional: terminal output cannot
prove that a reference belongs to the selected local folder rather than a remote
host. No file bytes are read on hover or Ctrl-click alone. Files may also be
selected directly from the local dropdown. Positive decimal location suffixes such
as `src/App.tsx:12` and `src/App.tsx:12:3` also select that same file; each
number must fit a nonzero 32-bit unsigned integer without leading zeroes. Clicking the suffix works too.
With a text preview open, **Open verified local file in Vim9…** opens a
separate native chooser. Choose the matching local file to start Vim. Aether
checks the selected name and raw-byte SHA-256 before launch. For a folder
snapshot it also checks the selected folder name and relative path, and passes
the confirmed line and byte column to Vim. Close an existing Vim workbench
before opening another file this way.
A literal numeric-colon filename in the selected snapshot (for example
`a.md:12`) takes priority over interpreting `:12` as a location for `a.md`.
Only one or two numeric colon suffixes on the final filename are admitted;
schemes, drive prefixes, colon-bearing directories and other colon forms remain
rejected. Supported text extensions may carry these literal suffixes. An unreadable
or disappeared literal file produces an error, never a read of its unsuffixed
sibling. Image names with literal colon suffixes remain unsupported.
Sentence-ending ASCII/Chinese punctuation is accepted only when the full
punctuation run ends at whitespace, a supported quote/bracket, or the row end.
For example, `README.md,` and `README.md:12。` resolve, but
`README.md.js` and `README.md,backup` do not partially resolve.
Literal punctuation in filenames is not stripped. If multiple authorized names
could explain the clicked reference (such as `README.md` and `README.md,`),
the click is rejected rather than guessed. Unsupported extensions participate
in this ambiguity check but cannot be previewed: `README.md,` is not an
enabled source extension, while `notes,draft.md` is. Clicking prose punctuation alone does
not select a file. Chinese quote/bracket wrappers are supported; unspaced
punctuation followed by more prose remains conservatively unmatched.

Only selected files are accessible, with a 256 KiB UTF-8 text bound.
Static PNG and single-scan baseline JPEG are also previewable, capped at 2 MiB
encoded bytes, 4096 pixels per side and 4 megapixels. Container admission runs
before native decoding; PNG uses a strict chunk allowlist (no animation,
compressed text/ICC or EXIF metadata), and JPEG rejects progressive/multiscan,
EXIF/ICC/multipicture extensions and concatenated images. Some otherwise valid
images are deliberately unsupported. PNG/JPEG extensions must match their magic.
Only one file read or decode may be pending at a time, including across close or
source changes. Late bitmaps are discarded and closed; canvas content is cleared
on dismissal. A WebView without createImageBitmap fails closed. These are byte,
dimension and concurrency limits, not a hard codec CPU timeout or replacement
for the browser's native decoder security. Native image decoding remains to be
validated in WebKit. Markdown,
TSX, HTML and SVG are displayed as literal source via `textContent`, never rendered
as HTML, executed, or opened with a shell. Closing, clearing/replacing the folder,
and newer requests invalidate pending reads; session changes invalidate terminal
candidates. There is no arbitrary filesystem path reader or shell command helper.
The directory chooser's snapshot is not live disk data; choose it again to refresh.

Terminal path matching in this local picker accepts visible, unwrapped relative
names in the selected folder. SSH project browsing has its own source and file
controls in **SSH Files**.
Node tests cover container admission and the actual controller with a model DOM, mocked decoder and File objects; native
WebKit directory chooser, physical Ctrl-click and screen-reader acceptance remain
unverified. A browser without directory selection fails closed instead of guessing
a root. Shared-core candidate parsing is a follow-up, not an API this slice assumes.
