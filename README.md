# Aether

Aether is an experimental Linux terminal built with Tauri 2 and the shared Rust
`jterm_engine`. Rust owns ANSI parsing, screen state and input encoding; the
WebView renders validated frames on Canvas. There is no xterm.js or JavaScript
ANSI parser. **This is not a daily-use terminal.**

## Shared core

The current exact pin is `f0107afaf631d29d06358abec3997f1b5712b627` in the
session, bridge and Tauri manifests and all three Cargo lockfiles. Run
`python3 scripts/check-core-pin.py` to check the pin and relative crate paths.

The updated core checks that a resized publication advances the layout revision.
Aether sends bounded full frames with screen and row-wrap metadata so the
WebView can keep text selection tied to the displayed grid. See [upgrade verification](docs/CORE-UPGRADE.md).

## Terminal interaction

Open the experimental shell and click the terminal to focus logical input.
Drag across the Canvas to select visible text, including wide cells and
soft-wrapped rows. A completed selection keeps the captured text after live
output replaces the displayed cells. Use the selection copy control to retrieve
that text.

A terminal application can request button, drag, motion, and vertical wheel
reports. Hold **Shift** to select text locally while application mouse mode is
active. On the primary screen, **Shift+Ctrl+left-click** can select an authorized
file reference in that mode.

Aether starts `/bin/sh` with `TERM=aether` and supplies its compiled ncurses
entry to the child. The entry describes color, alternate-screen, navigation
keys, bracketed paste, and application mouse capabilities. Shell sessions on
another machine need the Aether entry installed there to recognize the name.
See the [terminfo source](session/terminfo/aether.ti) and [ncurses format](https://invisible-island.net/ncurses/man/terminfo.5.html).

The PTY opens at the current viewport size and follows the window by default.
**Apply fixed size** uses the entered rows and columns. Select **Fit to window**
to resume automatic sizing. A resize briefly holds frame delivery while the
application repaints. The PTY continues to drain and the new grid appears once
the repaint settles or reaches its deadline.

## Agent Workspace

Expand **Agent Workspace** to load local Codex and Claude CLI session metadata.
When a same-user shared Workspace host is already running, Aether adds its API
chats, tasks and indexed history to the browser. Local CLI discovery uses
`CODEX_HOME` and `CLAUDE_CONFIG_DIR` when configured, or the corresponding
directories under the user's home. The shared host uses the family's workspace
state directory.

Combine type, provider, project, library, pinned and activity filters, or enter a
structured search such as `title:"release notes" -tag:draft`. The same filters
apply to the conversation catalog and indexed history. Selecting a conversation
shows its metadata. Selecting a history match shows the exact indexed event and
revision as read-only text. The history results describe the scope of the
shared index. **Refresh** reloads the catalog and **Clear filters** restores the
default active library.

Select a stopped saved CLI session and choose **Delete saved session** to remove
its shared Workspace entry and indexed history after confirmation. The original
provider transcript stays on disk. The operation requires a running shared host
and a current, identifiable CLI source. Deleted entries stay out of the catalog
after refresh and rediscovery.

## Build and check

Use Linux, Python 3.11+, Node 24 and the Rust toolchain specified in
[rust-toolchain.toml](rust-toolchain.toml). Install the official distribution
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). The CI workflow
uses Ubuntu 24.04 packages for GTK3, WebKitGTK 4.1, librsvg, libxdo, OpenSSL, Vim
and Xvfb. Install `ncurses-bin` for `tic`, which compiles the bundled terminfo.

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
cargo build --locked --manifest-path src-tauri/Cargo.toml --bins --examples
xvfb-run -a -s '-screen 0 1280x1024x24' dbus-run-session -- \
  timeout 55s src-tauri/target/debug/examples/native_smoke
# On a desktop with normally installed WebKit subprocesses and a working display:
npm run tauri -- dev
```

There are three independent Cargo manifests, not a root Cargo workspace.
`src-tauri` depends on `../bridge`, bridge on `../session`, and Tauri embeds
`../web`. Internal crate/package identifiers remain those of the imported lab;
they are not additional terminal engines.

## What validation covers

[Linux CI](https://github.com/beamiter/aether/actions/workflows/ci.yml) runs on
pushes to master, pull requests and manual dispatch. Check the run for the exact
commit; this link alone is not a claim that a particular commit passed.

- Node tests use modeled DOM/Canvas and IPC to exercise frame validation,
  selection, paste review, history, recovery and the file inspector.
- Rust session/bridge tests cover parsing, bounded state, real Linux PTYs,
  epochs, resize repaint, frame ACK/recovery, mouse reports and direct-child
  lifecycle.
- Tauri command tests use the actual registrations, a mock runtime and real PTYs.
- The Vim smoke uses the same terminal profile as the desktop shell. It checks
  screen transitions, edits, resize, Chinese committed text, exact saved
  content and primary-screen restoration through a real PTY.
- The WebKit smoke maps the Tauri window under Xvfb and checks PTY IPC, native
  window fitting, fixed sizing, Workspace search, exact evidence and deletion,
  source search, reload and PNG decoding. The Workspace smoke uses an isolated
  shared host and synthetic conversations. Preview file fixtures are supplied
  programmatically.

Wayland, platform IME, candidate positioning and broader accessibility require
desktop acceptance. Process ownership covers the direct shell.
See [lifecycle policy](LIFECYCLE-POLICY.md) and
[session contracts](session/README.md).

## History

Aether was imported from the v19 terminal laboratory checkpoint. Historical counts,
environment failures and archive-only log references are retained in
[lab history](docs/LAB-HISTORY.md), [validation history](VALIDATION.md), and
[import notes](PUBLISHING.md). They do not override the current pin, build commands,
CI result or acceptance limits above. The old Library archive is not the maintained
source; this repository is.

## Experimental local source preview

Choose a trusted local directory in the page to authorize a browser `File` snapshot
(up to 2048 files total, checked before iterating the FileList). For larger projects,
use **Single local file** to select the desired file directly without
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
as `src/App.tsx:12` and `src/App.tsx:12:3` also select that file.
After confirmation, source previews highlight the requested line and show the
reported column when present. Each number must fit a nonzero 32-bit unsigned
integer without leading zeroes. Clicking the suffix works too.
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

The inspector supports line navigation, literal search with an optional
case-sensitive match, previous and next occurrence, and copying the displayed
path, text, or current line. Search reports up to 4096 occurrences. Common
extensionless source files such as `Makefile`, `Dockerfile` and `LICENSE`
are available alongside source extensions. Text
previews read up to 256 KiB of UTF-8. The display normalizes line endings and
escapes unsafe control characters. Markdown, TSX, HTML and SVG appear as
literal source.

Static PNG and single-scan baseline JPEG are also previewable, capped at 2 MiB
encoded bytes, 4096 pixels per side and 4 megapixels. The image toolbar provides
fit, actual size and zoom from 25% to 400%. Container admission runs
before native decoding; PNG uses a strict chunk allowlist (no animation,
compressed text/ICC or EXIF metadata), and JPEG rejects progressive/multiscan,
EXIF/ICC/multipicture extensions and concatenated images. Some otherwise valid
images are deliberately unsupported. PNG/JPEG extensions must match their magic.
Only one file read or decode may be pending at a time, including across close or
source changes. Late bitmaps are discarded and closed, and dismissal clears the
canvas. Image decoding requires WebKit's `createImageBitmap`. Closing, clearing
or replacing local access invalidates pending reads. Session changes invalidate
terminal candidates.

**Reload** reopens the same file or folder picker for a fresh snapshot. When the
same preview is available, the inspector restores its search, line, scroll and
zoom context. The existing preview stays visible if the new read fails.

This slice does **not** support other image formats or remote file reads,
absolute paths, OSC 8/file URLs, wrapped filenames, or
editor navigation. It does not claim every Codex CLI output format is recognized.
Node tests cover folder admission, terminal-reference confirmation and PNG/JPEG
container checks. Directory chooser interaction, physical Ctrl-click and
screen-reader acceptance still require native coverage.
