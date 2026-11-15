# Browser editing input routing

The IME textarea is a typing channel, not a second clipboard transport.
Non-composing `input` events now accept only `insertText`. Paste continues through
`app.js`'s canceled `paste` event and epoch-bound review; drop, replacement,
undo/redo, line/paragraph insertion and unknown input types are discarded and
cleared instead of being sent as committed terminal text. Drag-and-drop text is
not currently supported. Composition commits still come from `compositionend`;
preedit and late composition input do not submit separately. Ordinary Unicode
text and null-data textarea fallback for `insertText` remain supported.

This closes an adapter bypass where `insertFromDrop` or `insertFromPaste` could
send multiline text directly through `text_shell`, without clipboard review.
The input-type restriction is frontend routing, not an IPC authorization boundary.

Validation: 64 Node tests pass. The new rejection and duplicate-preservation
regressions fail against the previous adapter. Existing composition order,
epoch/blur/reset, clipboard review and app lifecycle tests still pass. These are
synthetic DOM/IPC events, not native WebKit, platform IME or actual drag/drop
acceptance. No Rust files changed; local Cargo and GUI validation were unavailable.
