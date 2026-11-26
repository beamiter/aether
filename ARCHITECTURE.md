# Minimal vertical delivery path

## Authority boundaries

```
OS PTY -> bounded Rust reads -> jterm_engine Parser -> Rust semantic/session adapter
    -> immutable CellFrame -> bounded wire DTO -> Tauri command/channel -> Canvas paint
UI normalized intent -> session/focus admission -> engine keyboard/mouse/paste encoding -> PTY
Renderer ACK -> exact Rust in-flight frame identity -> next coalesced update
```

The WebView is not a VT authority. It receives only resolved rows/cells/cursor metadata and emits normalized user intent. Do not move ANSI handling into JS when a missing Rust behavior is inconvenient. A temporary limited headless adapter must explicitly list unsupported semantics and must eventually reconcile with existing production adapters.

Live full frames carry screen identity and soft-wrap flags from the engine.
During a drag, the renderer keeps the displayed grid stable and acknowledges
accepted same-layout frames into one latest-frame slot. A geometry or screen
change cancels the in-progress drag. Completed selections retain the text
captured at release.

Application mouse reports use the displayed session, layout and mouse mode
generation. Rust checks that identity against the live PTY state before
encoding and sending reports. The shared engine owns wire encoding.

The bridge resizes the PTY immediately and holds publication until the output
is quiet for 120 ms, up to one second per resize burst. Input delivery, PTY
draining, and synchronized-output accounting continue while publication waits.
Recovery during this window returns the last complete frame.

The renderer measures the viewport in terminal cells. Its epoch-bound queue
allows one resize request and keeps only the latest pending size. The host
validates geometry for both initial PTY creation and resize.

Agent Workspace uses the shared core's local session catalog, query parser and
same-user host client. The Tauri adapter exposes bounded catalog pages and exact
indexed evidence. Deleting a saved CLI entry requires a confirmation token
bound to that source. The shared host owns the catalog change and durable record
of the deletion. The standalone PTY and the conversation catalog keep
independent state.

## Contract next steps

- Host-assigned session identifiers and epochs: unknown/stale identity rejected; frontend does not select another process by arbitrary PID.
- Bound rows, columns, cells, input bytes, output batches, subscriber count and pending bytes before allocation.
- One in-flight frame plus latest-desired coalescing; do not queue one Tauri message per PTY read.
- Every delta carries exact epoch/revision/layout/base; full reset on mismatch, layout changes or attach. Validate atomically before drawing. The current prototype deliberately emits full frames only.
- Retain terminal state while renderer is detached, with explicit session lifecycle and admission policy. Do not use browser refresh as a process lifetime primitive.
- Resize is atomic host state + PTY geometry transition, not stretching the Canvas bitmap.
- Controlled keyboard/IME/paste pipeline: IME composition must not double-submit; bracketed paste uses core encoding. Clipboard and URL side effects stay host-authorized.

## Native-first acceptance gates

1. Build actual Tauri executable and display a real Rust frame.
2. Real fixed `/bin/sh` PTY: `printf`, prompt, Unicode basic width, CR/LF, scroll, resize and clean exit.
3. Escape semantics differential corpus vs chosen production baseline; fragment every input at adversarial boundaries.
4. Vim alternate-screen entry/exit, cursor, keyboard/mouse modes and replies. Unsupported cannot silently become “passed”.
5. IME CJK, combining/emoji, selection and clipboard, font metrics across DPI/zoom.
6. Stress output with slow/suspended renderer: bounded memory, latest-frame eventually visible, no stale epoch accepted.
7. Kill/reopen/window close and child descendants: ownership checks, no leaked shell, no accidental unrelated process kill.

## Desktop validation

Linux validation uses distro GTK3/WebKitGTK 4.1 and a private Xvfb display.
The mapped smoke exercises the built executable through native WebKit and IPC.
Wayland, platform IME, accessibility, and wider terminal workloads have their
own acceptance requirements.

## Separate legacy frontend work

Forge/Anvil VTE replacement is not a prerequisite for this vertical prototype. Their independent parser/renderer acceptance is a separate migration. Next reusable bounded replay extraction may benefit them, but must not delay Tauri indefinitely.
