# Validation record

- Rust stable 1.99.1, engine exact Git revision 57ed89a62fdd61db060a3c976ab9e8cdac9ae823.
- `session`: 4 unit tests pass, including actual fixed `/bin/sh` PTY printf, resize, output drain, exit and idempotent close. Strict Clippy passes.
- `bridge`: 3 unit tests pass, including actual shell lifecycle and exact single-flight epoch/ACK rules. Strict Clippy passes.
- Node: 7 tests pass, including immutable frame acceptance, cursor/layout bounds, Canvas paint calls, late old-session rejection, partial-send lockout, late demo response isolation.
- `bridge/examples/pty_frame.rs`: actual fixed command output becomes a Rust engine frame; exported JSON is accepted by JS and reaches Canvas paint calls. Prompt startup timing is accounted for explicitly, not confused with echoed command text.
- Tauri CLI 2.12.4 recognizes config. Native app not compiled/launched: missing GTK3, WebKitGTK4.1 and display server in this cloud workspace.
- No native UI or pixel acceptance. Cloud-browser loopback preview attempt failed with connection refused; this is not a page correctness result.
- No remote repository created. No commits or code pushed. No user Codex thread accessed.

## Native compilation checkpoint (2026-11-12 UTC)

- Official Debian trixie snapshot GTK/WebKit development packages and Xvfb downloaded and unpacked only under `native-deps/root`; system unchanged.
- Full Tauri 2 desktop executable compiled successfully with Rust stable 1.99.1, one build job and independent target directory. Resolved versions locked in `src-tauri/Cargo.lock`: tauri 2.12.5, tauri-runtime-wry 2.12.3. Fixed the previously missing default application icon exposed by the first real compile.
- WebKit subprocess dynamic-library resolution passes under the workspace-only runtime wrapper.
- Native launch attempted but GTK initialization fails because no display is available. Authenticated local Xvfb cannot open listeners; even creating an AF_UNIX socket returns EPERM in this container. The supported elevated invocation yielded the same failure. No sandbox disabling or system security changes attempted.
- Therefore native compilation is PASS, native window rendering/interactions are NOT VERIFIED. The PTY/JSON/Canvas-call tests are not a substitute for real WebKit/GTK acceptance.

## Session hardening checkpoint

- Rechecked selectable environments: only this cloud computer; no alternate cloud GUI is available. No user-local or Codex thread was used.
- Session now retains bounded read-side draining through write/reply failure and child exit; write errors are sticky, reply loss is counted, `is_drained` requires exit plus PTY EOF. Bounded EINTR retry and reply queue overflow tests added; close cleanup is retryable.
- Rust bridge exposes epoch-scoped health (unsupported events, exit/drained, write failure, lost reply bytes). UI locks input on failure/exit, keeps draining until done, and ignores stale async status/resize results. Failed close retains epoch for cleanup retry.
- Session Rust: 9 tests plus strict Clippy; bridge Rust: 3 tests plus strict Clippy; Node: 13 tests, all pass. Native GUI remains unverified for the display/socket restriction above.

## Real Vim backend milestone

- Added host-side ED, insert/delete lines, region scroll, IND/NEL/RI, DECALN, REP and alternate-screen modes 47/1047/1048/1049 using existing `jterm_engine@57ed89a` ScreenState methods and Ember host mappings. RowMutation effects describe edits already applied by the shared core and are not applied twice.
- Actual official Debian Vim 9.1.1230 (+security patches) over a real PTY passed alternate entry, insert rendering, resize, 36-line scrolling, delete/undo, write/quit, exact file contents and primary restoration. See `real-vim-results.txt` and reproducible example. This is not an Ex/batch-mode test.
- 18 unsupported sequences remain: bounded identifiers show title-stack CSI t, application-cursor mode, bracketed paste/focus/cursor blink, DA2 and OSC/DCS queries. No full compatibility claim; unsupported identifiers deliberately omit OSC/DCS payload text.
- Rust-owned cursor visibility now reaches Canvas, with a hidden-cursor regression.
- Final scope: session 15 tests, bridge 3 tests, Node 14 tests pass; strict Rust Clippy passes. Tauri native executable compiles. No GUI validation: display/socket restriction unchanged, and UI input is still line-oriented.

## Logical keyboard and paste milestone

- Focused Canvas keydown/paste events are forwarded as logical data, never JS-generated ANSI. Composition/dead keys are withheld, not claimed as IME support.
- Rust uses shared function-key and modifyOtherKeys encoding. Modes 1/2004 drive application arrows and bracketed paste; unsupported Kitty flags remain rejected explicitly.
- Added a 16 KiB atomic-admission host queue, bounded polling with partial-write suffix retention, and per-epoch UI queue isolation (128 pending logical events maximum). A stuck old promise cannot block a reopened session.
- Real Vim smoke now also asserts bracketed paste markers from Rust, actual SS3 left/right movement and resulting cursor columns. Exact saved-file comparison still passes. Unsupported count is now 13 (remaining title/focus/blink/query families).
- Session 18 tests, bridge 4 tests, Node 18 tests and strict Clippy pass. Updated Tauri executable builds. Native GUI/keyboard routing/IME acceptance remains unverified due to the same display restriction.

Independent review caught command-submit racing queued logical keys. Submit now awaits the admitted logical-input chain, separately tracking permanent input lock; a delayed-IPC regression verifies both queued keys precede line submission without loss. String invoke errors are now displayed correctly.

## Chinese composition milestone

- Added an editable textarea for desktop composition; preedit remains local. Committed UTF-8 is sent once through `text_shell` and bounded Rust `encode_text`, never decomposed into key events or paste markers.
- Event-model regressions cover compositionend before/after final input, cancellation/blur, stale epoch/reset, duplicate suppression and the next identical intentional character. Integration test verifies exactly one Chinese text IPC and drops late close-session commit.
- Real Vim exact-file test includes `中文输入测试` through the committed-text encoder, alongside prior paste/application-arrow/resize/edit tests. This is a backend Unicode test, not a real OS IME acceptance.
- Session 20, bridge 4, Node 24 tests pass; strict Clippy/native build pass. Candidate placement currently anchors at textarea, not terminal cursor. Native WebKit/IME and mobile event sequences remain unverified.
- Remaining 13 unsupported events: prioritize identifying DA2/DCS query subtypes; no speculative capability replies. Focus reporting needs actual GUI focus ownership, titles require classified OSC/window-host integration, blink is lower priority.

Independent read-only IME review found no definite duplicate text/key or cross-epoch delivery in the modeled paths. Real WebKit acceptance must still check candidate-confirm Enter ordering (compositionend versus an unmarked Enter keydown), blur/end ordering and candidate positioning. These are not resolved by synthetic tests.

## Query safety checkpoint

XTGETTCAP fixed negative response passed an actual shell PTY exact-byte roundtrip, with one response and zero lost reply bytes. Real Vim smoke remains PASS; unsupported=14 is intentional after fixing the intermediate-SGR probe instead of falsely treating it as supported SGR.

Independent security review found no payload reflection or capability spoofing. Added reviewer-suggested query flood/cancel/oversize tests. The oversized test exposed that parser pending_bytes is not a complete-event size cap; the adapter now explicitly gates capability replies at 4096 payload bytes. Session: 23 unit + 2 integration tests; bridge: 4; Node: 24. Native GUI remains unverified.

## Bounded history / manual copy batch

Primary full-screen upward scrolls now retain up to 1024 shared-core compressed
physical rows. Alternate-screen output and partial scroll margins do not enter or
expose that history. Historical width is retained after resize; no reflow is claimed.
A separate immutable history snapshot (at most 128 rows, 131200 UTF-8 bytes) is
served under the session lock, epoch scoped, and never changes live frame ACK state.
The UI has older/newer/refresh controls and a readonly text area for ordinary manual
selection and copy. Snapshot text remains frozen while PTY output advances. Opening
or closing a session clears it; stale IPC responses cannot cross epochs or overwrite
newer requests. Visible-grid snapshot is separately available, with visual line
breaks, not semantic reflow. No system clipboard permission/API was added.

Canvas backing allocation is capped at 16 megapixels even at maximum viewport
and high device DPR. The frame receiver rejects C1 controls and isolated UTF-16
surrogates, while accepting actual astral scalars. This does not add glyph shaping.

All new UI validation is Node/model based. Native build success is not native GUI,
mouse-selection, scrolling or clipboard acceptance; the cloud display blocker
remains unchanged.

History batch verified: 28 session unit + 2 query integration tests, 4 bridge tests,
29 Node tests. Session tests include real fixed /bin/sh PTY multi-line output,
exit/drain and exact history content. Existing interactive Vim test re-run passed
(14 unsupported identifiers unchanged), including Chinese text, bracketed paste,
resize, edit/undo, saved-file bytes and primary restoration. Native Tauri build
also succeeded. None of these statements claim GUI display acceptance.

## Lifecycle / pressure recovery batch

The live bridge now exposes explicit full-snapshot recovery for a lost frame/ACK.
It replaces only transport flight state, never the PTY or queued input. An exact
ACK retry after successful acknowledgment is idempotent, but a stale ACK cannot
acknowledge a newer flight. UI recovery invalidates outstanding poll callbacks and
restarts bounded polling. Existing input safety locks (notably partial-write
uncertainty) remain locked; recovery does not automatically resend any command.

A real fixed /bin/sh PTY produces 1400 lines plus FINAL_SENTINEL and exits while
its initial frame ACK is deliberately withheld. Polling still drains all output,
history stays capped at 1024 rows, and an explicit recovered full frame contains
the exact final sentinel. Stale ACK rejection, retry ACK and foreign epoch rejection
are exercised. Node tests model an old poll failing after recovery and recovery
after partial command failure. These are transport/PTY tests, not native GUI tests.

Historical pre-supervisor limitation (superseded by the next batch): explicit Close/Drop invoked portable-pty kill (up to its
roughly 200 ms HUP grace) followed by blocking child wait with no deadline. Tested
normal exit/close is not a guarantee against an uninterruptible child. Introducing
unbounded per-child reaper threads would trade this for another resource problem,
so this batch does not do that. A bounded shared reaper and explicit closing state
remain necessary before claiming production-grade nonblocking shutdown. Only the
owned direct child is managed; descendant process containment is not implemented.

Current pressure/lifecycle batch: 32 session unit + 2 query integration, 5 bridge,
31 Node tests pass (70 total). Session tests include 3 real-PTY 70KB output/exit-tail
runs and 8 repeated live-close runs. Synthetic short-read chunks exposed and now
regress a poll budget overshoot: each read is clipped to the remaining 64KiB budget.
Native Tauri compilation succeeds; the GUI remains unverified.

## Bounded direct-child supervisor batch

Close/Drop now hands the owned direct child to one global reaper instead of using
blocking wait or portable-pty's kill grace. A slot is reserved before any PTY child
spawn, at most 32 active/pending slots; unresolved cleanup prevents slot reuse.
Signals use a Linux pidfd only. Default SIGCHLD disposition and pidfd capability
are checked without changing system signal/security settings. Unsupported hosts
fail closed rather than using guessed/reusable PIDs. Errors retain ownership and
observe-only quarantine, not an assumption that a child is still alive.

CloseStatus distinguishes pending cleanup from confirmed direct-child reap. This
is deliberately not process-tree containment: a child that escaped into another
session is not discovered or killed. Parent app shutdown and native GUI behavior
still need platform acceptance. There is no per-child worker, no wait() call in
close/reaper, no unbounded task queue and no new process-group signalling.

Supervisor batch verification: 39 session unit + 2 query integration tests,
5 bridge tests and 32 Node tests pass (78 total), strict Clippy and native Tauri
build pass. A real owned shell explicitly ignores HUP and runs a shell-only busy
loop after an unambiguous output marker; pidfd cleanup reaches confirmed reap.
Other cases cover failure to start worker, full capacity, failed setup reservation
rollback, wait/signal quarantine, poisoned table, worker panic, slot reuse and real
PTY exit-tail draining. The interactive Vim regression still passes with 14 known
unsupported identifiers. Native GUI behavior remains unverified.

## UTF-8 / wide-cell / color batch

Wire schema v2 carries Rust-owned paint spans (0 continuation, 1 normal/clipped,
2 wide lead) and bounded style bits. Canvas paints all backgrounds before glyphs,
clips wide leads to two cells and never paints continuations as duplicate text.
A core-supported last-column truncated wide lead is preserved with paint span 1;
we do not discard a character just to fit the wire contract. Resizing follows core
wide-pair cleanup. Immutable visible-grid copy omits continuation padding.

Tests exhaust UTF-8 feed split points, wide-cell half overwrites, wrap/nowrap,
resize pair cleanup and real PTY Chinese output. The actual Rust Unicode fixture
is consumed by JS frame validation/paint-call tests, checking two-cell clips,
background-before-glyph order, exact RGB/256-color/inverse and style bits. Bold,
italic, dim, strike and a single underline are painted. All non-none underline
styles currently fall back to single underline; blink is not animated. Actual font
fallback/glyph metrics, antialiasing and GUI rendering remain unverified.

Combining accents, variation selectors and ZWJ/grapheme sequences are NOT retained
by the shared scalar-cell model. Zero-width scalars now increment a bounded fixed
unsupported diagnostic, preserve the base scalar and do not move the cursor. This
is explicit partial support, not grapheme correctness. Full support requires a
shared-core grapheme representation rather than a second JS terminal state.

Batch verified: 44 session unit + 2 query integration, 7 bridge and 35 Node tests
(88 total); strict Clippy, native Tauri build and existing actual interactive Vim
regression passed. GUI remains unverified.

## Cursor / resize / scroll batch

Wire v3 carries Rust cursor shape and pending-wrap state. Canvas paints outline
block, two-pixel beam or underline at exact cell coordinates; a cursor over a wide
lead spans both cells (except beam). The UI reports grid size and 1-based cursor
coordinates, hidden/wrap-pending state, and offers Show cursor to center/clamp the
live grid within a scrollable canvas container. Scrolling does not mutate terminal
state. Archived text is separate and labels the exact retained page range.

Columns 1–256 and rows 1–128 are user editable. Frontend resize retains at most one
active IPC plus the latest pending request per epoch; old responses cannot report
into a new session. Failure clears pending work rather than blindly retrying an
uncertain resize. Rust preflights dimensions/counters before kernel ioctl, then
applies shared geometry only on success. Real PTY tests check kernel winsize at
1×1 and 128×256, cursor shapes/visibility/positions, alternate restoration and
failure atomicity. Blink state is not present in the pinned core and not animated.

47 session unit + 2 query integration, 8 bridge and 41 Node tests pass (98 total),
strict Clippy, native Tauri build and actual Vim regression pass. Canvas geometry
checks are mocked paint-call/DOM geometry tests, not native GUI acceptance. Native
scrollbar behavior, font appearance and window interactions remain unverified.

## Bounded core checkpoint (2026-11-13 UTC)

See BOUNDED-CORE-CHECKPOINT.md for the new exact pin, 102 passing tests, strict Clippy, and explicit native acceptance limitations. Earlier counts and native/Vim logs above describe previous versions, not this checkpoint.
