# v18: bounded immutable history requests

History buttons previously issued a separate IPC per click. Generation checks
discarded stale responses but did not bound allocation or backend work.
History now admits one active request and one coalesced latest requested offset
per session epoch. Rapid older/newer requests accumulate toward the desired
offset, bounded to 0..1024. Superseded pages do not disturb the existing text
selection. Backend-clamped offsets become the next navigation base.
Reset/new sessions detach old in-flight work; old responses cannot overwrite
the new session. Errors retain displayed text, discard queued work and require
an explicit retry. This is a read-only snapshot path, not input replay.

Node suite: 60 / 60 passed, no skips. Added burst (200 extra clicks), navigation
coalescing, failure/retry, clamped offset and old-session tests. The burst test
fails against v17 (201 requests) and passes after the fix (one active request
then one latest request). No Rust/pin/locks/permissions changes. Cargo is absent;
native Tauri GUI, platform IME and real terminal performance were not tested.
