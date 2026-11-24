# Native local-shell WebKitGTK smoke

Run **Desktop WebKitGTK acceptance** manually on Ubuntu 24.04. It also runs
when a push to `master` changes the native workflow, a driver script, local
terminal source, WebView source or any of the three Cargo lockfiles. It builds and hashes the exact
checkout’s Aether desktop binary and verifies that the embedded Aether page,
not MiniBrowser or a mock runtime, is under automation.

The official [Ubuntu WebKitWebDriver](https://manpages.ubuntu.com/manpages/noble/man1/WebKitWebDriver.1.html)
listens only on a temporary loopback port. Its native
[GTK binary capability](https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebDriver/gtk/WebDriverServiceGtk.cpp)
receives that binary directly. Tauri's first-WebView automation permission is
enabled only in the private test launcher environment. There are no product
config changes, proxies, elevated browser privileges, or sandbox overrides.
Authenticated Xvfb and a private D-Bus session isolate the test, and the app
has an empty HOME, XDG directories and fixed shell fixture paths.

The smoke clicks the actual local shell controls, then types fixed commands
through WebDriver. Each owned local shell must show its own structured marker
and a distinct color in the actual WebKit canvas backing store after receiving
its frames over the native Tauri Channel. A subsequent
PTY update changes one row's color without disturbing the original marker or
background, and the change survives reattaching to that shell. The test
checks that an ambiguous roster preselects neither shell and disallows a third
open, switches both attached views from the roster, checks isolated drafts and
disabled input and resize, and verifies that no draft command executed. It
closes both selected local shells and waits for each distinct recorded direct-child
identity to disappear. WebDriver closes the session; the launcher also owns
bounded process-group cleanup.

The writable-tab journey opens two new local shells through the tab controls.
It submits input after switching in each direction and after closing a tab.
Each write comes from the same recorded child, drafts stay with their shell,
and closing the last tab reaps the surviving child.

The split journey checks two side-by-side Canvas surfaces, presents new output
from the inactive PTY, changes only the selected PTY's geometry, and transfers
keyboard focus to the other shell. Frame colors and PTY identities remain
distinct through the focus transfer.

The history journey searches text produced by a real PTY and selects the
newest matching text in the native textarea. **Older match** reaches a prior
history window, and searching the same term in the other shell finds no match.

The restart journey closes the desktop with two writable tabs and the first
selected, waits for both original child identities to disappear, and launches
the same binary in the same private profile. **Restore shell tabs** opens two
fresh writable PTYs, restores the split, and selects the first tab. Both new
child identities are distinct, the old draft stays out of the restored view,
and explicit close reaps both children. With the jsh fixture, both X11 and
Wayland verify that the second tab restores as a jsh with Terminal Chat
selected and Workflows available at its prompt.

Add `--jsh /absolute/path/to/jsh` to the driver invocation to include the
command-block journey. The isolated application PATH resolves that executable
through the normal Aether launch path. The journey exercises the real jsh OSC
133 markers, plain-text copying, hidden-input isolation, Vim on the alternate
screen, and session-local records while switching split-pane focus. It
requires Vim and uses the same WebKitGTK driver, private display and exact
binary as the rest of the smoke.
It recalls a command into the editable jsh prompt, appends another expression
before submitting, and checks that a foreground process waiting for hidden
input receives only the explicitly typed reply.

The same option activates the Terminal Chat journey. It switches the view
around a partially typed command, checks that Send reaches the existing prompt
once, and verifies the renderer, shell PID and working directory across the
switch. The test also covers automatic completed cards, hidden foreground
input, Vim's alternate screen and writable fallback for an ordinary shell.
The workflow builds a pinned jsh fixture for X11 and Wayland. The Wayland
driver's `--jsh` option additionally verifies that Send stays disabled during
native composition and that the committed Unicode reaches the PTY once on Send.

The jsh fixture also loads its workflow library from private TOML and YAML
files. It verifies that user configuration takes precedence over the additive
override, checks tag search and required and default arguments, and edits the
rendered line in the same PTY before pressing Enter. Switching tabs dismisses
the form for the previous attachment. The X11 and Wayland journeys also search
completed command cards and select the exact text after a non-BMP character
in the captured output. They bookmark that card, switch away from its tab and
back, remove and re-add the bookmark, and verify that retention eviction clears
it from the bookmarked-only view.

Add `--companion-daemon /absolute/path/to/jcompaniond` to the Wayland driver
with `--jsh` to run the Eva journey in a private daemon profile. Run the
driver under `xvfb-run` so Sway can use its X11 output backend and provide
presentation feedback. Sway disables XWayland and Aether itself uses native
Wayland. The driver observes
the native GTK slot through AT-SPI and checks the coordinator's presentation
and retirement records. Aether uses `wp_presentation` feedback for the
exact Wayland surface commit. The journey sends successful and failed commands
through two real jsh PTYs, switches the split, closes the first tab, and
verifies that the surviving tab starts with fresh feedback. Closing the native
window waits for the same body token to be retired. The desktop workflow
uploads the desktop and daemon hashes with the journey result.

To run the same journey on X11, build JWM with `backend-x11rb` and pass the
three fixture executables. The test starts JWM with software GLX compositing
on a private Xvfb and checks its extended frame timing. Repeat with
`--without-frame-feedback` to verify that jsh stays usable while the shared
body remains unplaced. `--peer-forge /absolute/path/to/forge` also verifies
the same Eva identity moves to Forge and back, with both previous tokens
retired before the next host presents it.

```sh
xvfb-run -a -s '-screen 0 1600x1200x24 -nolisten tcp' \
  dbus-run-session -- /usr/bin/python3 -B scripts/test-x11-companion.py "$desktop" \
  --daemon "$jcompaniond" --jwm "$jwm" --jsh "$jsh" --output "$evidence"
```

Build the native desktop from the Aether checkout using its published core pin.

```sh
python3 scripts/check-core-pin.py
cargo build --locked --manifest-path src-tauri/Cargo.toml --bin tauri-terminal-lab
```

Add `--notebook` to select an actual local `.jtnb.md` input through the
WebKit file control and verify its exact SHA-256. The journey explicitly
runs a shell fence, checks separate stdout and stderr and exit code 17,
then stops another fence after partial output arrives. It checks the
stopped process group's child never writes its delayed marker and reopening
the same selected file leaves every cell idle.

Add `--output-review` together with `--jsh` to compare two completed local
PTY blocks, generate enough new commands to evict both original blocks, then
download the frozen Markdown and JSON. The fixture reads both files and
checks the original revisions and diff after eviction.

Add `--vim9` with Vim 9, `xdotool`, ripgrep and the Python AT-SPI bindings installed
to select a real file in the GTK chooser. The fixture verifies the entered
path and activates the dialog's Open button through its accessibility interface.
It checks authenticated Vim9 Canvas painting,
edits that file through the buffer view and the fallback PTY, checks exact
Unicode bytes after native saves, and verifies Vim exits and its view clears.
It then closes the reopened Vim's GUI channel and saves through the same
editor's live terminal to verify the automatic fallback.
The same fixture navigates a project tree and a UTF-8 byte column, splits a
window from Vim's palette, searches into quickfix, observes build output and
exit code 7, and stops a running build. It also selects a text preview, rejects
a different file with the same basename, then opens the matching snapshot in Vim.
It samples distinct plugin popup and completion-menu colors from the native
Canvas, dismisses the popup, and verifies the chosen completion in Vim's buffer.

Use `--vim9` with `scripts/test-wayland-ime.py` to run the workbench on a private
Wayland compositor with XWayland disabled. The fixture checks one native
input-method commit, severs the Vim bridge, and checks the process identity
and PTY before saving from the live terminal. The workflow preserves its
`result.json` with the app binary digest.

Add `--workspace` to include the shared CLI Workspace journey. It starts the
embedded protocol-2 host with an isolated schema-6 SQLite directory, two
synthetic provider-history files with the same native UUID, and a controlled
local provider script in a real PTY. An existing embedded terminal holds the
lease first. Aether sees the conflict, attaches after release, and displays the
same provider output. The journey verifies explicit detach, closing the app
while attached, and reopening that same provider. The fixture splits a wide
cursor-position query across the legacy detach and checks that the Host
answers once before Aether attaches. It also checks the live query and reviewed
bracketed paste byte for byte. The final marker must appear with the exit
receipt.

The source-qualified CLI removal check uses two entries with the same title
and provider UUID. It cancels once, confirms removal of the idle source, and
refreshes the catalog before reattaching the still-running provider. It
compares both fixture transcripts byte for byte and emits
`PRODUCT.CLI.001.NATIVE.001` with the display backend and desktop SHA-256.
`--workspace-only` runs this journey independently. For native Wayland,
use `--workspace-only --backend wayland` with a private compositor, an
active keyboard seat, and its socket in `WAYLAND_DISPLAY`.

Add `--api-chat` to run an API conversation against a private loopback
streaming provider. The real Workspace host acknowledges a multiline draft,
rejects a competing controller, a stale draft save, and a duplicate
submission while the first request streams. Reopening Aether recovers that
same stream and the newer saved draft. The fixture releases the stream,
reads the completed exchange from SQLite, and checks explicit cancellation
and draft restoration on a second submission.

Add `--native-task` for the scoped native Codex approval journey. It requires
a working user systemd manager with delegated cgroup support. It uses a
disposable Git repository, a deterministic local app-server fixture and the
real Workspace native runtime. The test closes Aether during a pending
approval, reopens the same task, verifies one explicit decline passed to the
provider, releases the scope and reads the isolated worktree diff. It starts
the separately authored verification command, reopens during the running
verification without executing it twice, and checks that a worktree change
after inspection blocks acceptance. A fresh verification and inspection lets
the host accept the task. Aether reopens the same completed task.
The fixture then saves and reopens task context, checks shared revision
conflicts and redaction, and previews a Codex handoff. Opening the draft
preserves the source task and waits for separate consent and start. The
second scoped provider receives the reviewed text in its own worktree.
The follow-up has no verification recipe. Its human acceptance rejects edits
made after the first inspection and persists after inspecting and accepting
the changed worktree.
The Mission journey links the exact native worktree and run, rejects a stale
plan edit, inspects real local Git evidence, freezes the observed clean
candidate and retains the older verification as stale. After reopening, the
saved report requires fresh inspection to confirm delivery.
The Tools journey pages through the actual shared index to real verification
output. It rejects an imported record replaced since search, then compares
the opened validation text and exact revision with the host receipt.
The queue journey saves two authored drafts before provider launch, observes
dependency blockage, rejects an older revision, and reopens the saved draft.
An accepted dependency enables an explicit launch of that same task and
worktree. The other draft remains idle.

This scope is real Linux WebKitGTK/X11 under Xvfb, not a physical pointing
device, a compositor screenshot comparison, IME or Wayland coverage. The
normal mock-window PTY pressure regression separately covers detached output
beyond kernel capacity. No SSH host or user shell profile is used.

The smoke sends real SGR and wide text through a local PTY. It checks the exact
two double-underline pixel rows in the native Canvas, changes the explicit
color without changing the glyphs, and copies a line across a soft wrap.
An isolated desktop URI handler records one explicit OSC 8 Ctrl-click. The
test also verifies that hovering and a rejected userinfo target launch nothing.

`scripts/test-wayland-ime.py` exercises the same desktop build in a private
Sway session with XWayland disabled. It checks that preedit appears at the
visible terminal caret, the compositor receives a matching text-input-v3
rectangle, and the PTY receives the committed character exactly once. Its
second stage opens two real PTYs and cancels a pending preedit when switching
tabs. Fresh input reaches the newly selected shell. The input-method fixture
is built from Frost's `scripts/native-ime-client.c`.

## SSH Files and remote Workspace

Build the desktop and the `aether-terminal-host` helper. Supply their absolute
paths to the driver. It uses WebKitWebDriver, bubblewrap, an OpenSSH client
and server, and a private D-Bus session. The X11 variant needs Xvfb. The
Wayland variant starts a headless Sway compositor with `wtype` and XWayland
disabled. If using an extracted Sway build, pass its library directory with
`--compositor-libraries`.

```sh
dbus-run-session -- xvfb-run -a -s '-screen 0 1440x1100x24' \
  python3 scripts/test-remote-project-webkit.py "$desktop" "$helper"
dbus-run-session -- python3 scripts/test-remote-project-webkit.py \
  "$desktop" "$helper" --backend wayland
```

The fixture pins an ephemeral loopback host key, creates separate local and
remote Workspace stores, and reads back the actual downloaded bytes. It checks
same-name local and remote files, changed file versions, directory navigation,
binary transfer and no-overwrite upload. It checks the expected host ID, both
kinds of pagination, stale search selections, and the document's exact revision.
Successful output includes the case ID, display backend and both binary hashes.
The hosted workflow uses `--runner-ssh-config` on its disposable account when
the fixture cannot depend on a user namespace. The option reserves an empty
`~/.ssh/config` slot for the ephemeral host and removes the link on exit.
It refuses to replace an existing configuration. Local runs use bubblewrap.
The desktop workflow runs both displays against the same build. Its X11 local
journey also exercises Notebook and the Vim9 workbench.

## Workspace upgrade and backup rollback

Run the two built desktop executables through their embedded Workspace hosts.
The rehearsal creates a disposable state root with the previous build, saves a
draft, and takes an online SQLite backup. The candidate opens the original draft
and saves a new revision. The previous build then opens and edits the backup in
a separate state root. The candidate reopens its own root and verifies its newer
draft. A third copy carries a newer schema version so the rehearsal can check
the candidate's refusal and compare the database bytes before and after.

```sh
python3 -B scripts/test-workspace-rollback.py "$previous_desktop" "$candidate_desktop"
```

The result records both executable hashes and the refused schema version under
`WORKSPACE.ROLLBACK.001`. Use two distinct builds that support protocol 2.
