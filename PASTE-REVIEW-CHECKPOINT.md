# v15: epoch-bound multiline clipboard review

Date: 2026-11-14 UTC. Supersedes only the UI clipboard behavior in v14.

## Implementation
- Added `web/paste.js`, integrated with `web/app.js` and a readonly review region.
- Every multiline clipboard offer (LF, CR, CRLF) requires a dedicated send click.
- Preserve original text exactly for Rust encoding; do not reflect it as HTML.
- Pending text belongs to one session epoch. Reoffer supersedes it, even when new input is invalid or empty. Confirm consumes it before enqueueing, preventing double sends.
- Close/open, exited/writer-error status, poll failure, input queue overflow and logical/command write failure clear review state.
- Control rejection matches the existing Rust clipboard policy (TAB/CR/LF allowed), and the UTF-8 input ceiling is 4096 bytes. Rust can still reject the encoded wire size after adding bracketed markers; no mode prediction is made in JS.
- Single-line clipboard input remains direct. Explicit Send line and keyboard Enter are intentionally unchanged.

## Validation in this checkpoint
`npm test`: 50 passed, 0 failed, 0 skipped (see `paste-node-results.txt`).
Added 7 focused review tests and 2 actual app-script lifecycle harness tests.
Covers CR/LF/CRLF, Unicode/tab exactness, cancel/reset, one-shot confirmation,
stale/unavailable session, superseding offers, byte boundary, control rejection,
close/reopen and process exit while a review is pending.

## Limitations
This is UI routing tested in Node using DOM/IPC stubs, not actual WebKit mouse or
clipboard acceptance. No Rust or lockfile changes. Cargo is unavailable in this
execution environment, so no new native build, Rust tests, PTY/Vim smoke, GUI,
X11/Wayland, accessibility-reader or platform IME acceptance is claimed.
The historical v14 native/Vim evidence remains unchanged and dated.
This is not backend IPC authorization or shell-command safety analysis; approving
multiline input can execute commands, including when bracketed paste is disabled.
The prototype remains non-production: TERM=dumb, incomplete VT/grapheme coverage,
no process-tree containment, no Kitty image assembler.
