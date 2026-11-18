# Aether

Aether is an experimental Linux terminal built with Tauri 2 and the shared Rust
`jterm_engine`. Rust owns ANSI parsing, screen state and input encoding; the
WebView renders validated frames on Canvas. There is no xterm.js or JavaScript
ANSI parser. **This is not a daily-use terminal.**

## Shared core

The current exact pin is `8d0c0aafa06468b3935712fa477a971bc3cd8eae` in the
session/bridge manifests and all three Cargo lockfiles. Run
`python3 scripts/check-core-pin.py` to check the pin and relative crate paths.

Compared with the imported `2b37583` checkpoint, this revision adds parser
conformance coverage, corrects shared mouse report encoding and checks frame
delta budgets before scratch allocations. Aether currently sends full frames and
does not forward mouse reports, so those core fixes do not add mouse support or
delta transport to this UI. No adapter API changes were required. See [upgrade verification](docs/CORE-UPGRADE.md).

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
- Linux compilation links a debug desktop executable; it does not launch a GUI.

GUI pixels, native keyboard/clipboard/composition, Wayland, platform IME and
candidate positioning remain unverified. Process ownership covers only the direct
shell, not descendant containment. The product shell uses `TERM=dumb`; the Vim
smoke opts into `TERM=xterm`. Unsupported sequences and Unicode shaping limitations
remain explicit. See [lifecycle policy](LIFECYCLE-POLICY.md) and
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
This opens source preview only, not an editor jump or line highlighting.

Only selected files are accessible, with a 256 KiB UTF-8 text bound. Markdown,
TSX, HTML and SVG are displayed as literal source via `textContent`, never rendered
as HTML, executed, or opened with a shell. Closing, clearing/replacing the folder,
and newer requests invalidate pending reads; session changes invalidate terminal
candidates. There is no arbitrary filesystem path reader or shell command helper.
The directory chooser's snapshot is not live disk data; choose it again to refresh.

This first slice does **not** support raster image decoding, remote file reads,
absolute paths, OSC 8/file URLs, wrapped filenames, or
editor navigation. It does not claim every Codex CLI output format is recognized.
Node tests cover the actual controller with a model DOM and File objects; native
WebKit directory chooser, physical Ctrl-click and screen-reader acceptance remain
unverified. A browser without directory selection fails closed instead of guessing
a root. Shared-core candidate parsing is a follow-up, not an API this slice assumes.
