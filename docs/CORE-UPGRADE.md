# Shared core upgrades

## f0107af

The engine and desktop Workspace client use
`f0107afaf631d29d06358abec3997f1b5712b627`. The shared host adds the
confirmed saved CLI session deletion command and records deleted source
identities across catalog rediscovery. The original provider transcript stays
on disk.

## 8e904f3

Aether moved to `8e904f3b10b3235ff225f37227866f6f5ef4024f` in its git
dependency declarations and all three Cargo lockfiles. The published
`jterm_engine` adds a publication guard for dimensions that change without a
layout advance. The Aether session and WebView also check this invariant across
real PTY resizing and full-frame admission. The session's Unicode width
requirement now matches the 0.2.3 version in all three locks.

The full-frame wire carries engine-owned screen identity and soft-wrap flags.
Legacy version 3 browser fixtures remain readable. The input and rendering
paths still share the same Rust engine.

The desktop Agent Workspace locks `jterm_core` to the same revision for its
conversation catalog, search filters and read-only host protocol. The pin guard
checks both packages in the desktop lockfile.

## 8d0c0aa

Validated against Aether import `49963b4da6b99ab49a410c25a7451bcce7888491` on
2026-11-14 UTC. The exact new engine revision is
`8d0c0aafa06468b3935712fa477a971bc3cd8eae` (formerly `2b37583`).

The two Git dependency declarations and three lockfile source identities change
together. No other dependency versions or adapter APIs change. Relative paths
remain src-tauri → ../bridge → ../session and frontendDist → ../web.
`scripts/check-core-pin.py` checks the core and crate path invariant in CI.

## Local repository verification

Executed in the actual Aether checkout with official Rust 1.100.0, using existing
workspace-extracted official Debian GTK/WebKit libraries for native compilation.
No system packages or sandbox settings were changed in this verification.

| Check | Result |
| --- | --- |
| Node contract tests | 60 passed |
| session tests | 57 passed (51 unit, 2 query/PTY, 4 shell contract) |
| bridge tests | 8 passed |
| Tauri command tests | 2 passed; mock window runtime with real PTYs |
| All three crates: fmt and Clippy, all targets, warnings denied | passed |
| Linux desktop debug build, locked dependencies | passed |
| Actual Vim PTY smoke | passed; 15 unsupported events remain |
| Shared pin/path guard and workflow YAML parsing | passed |

The Vim smoke checks alternate screen, rendering state after insert, bracketed
paste, application arrows, Chinese committed text, resize, scrolling/edit/undo,
exact saved content, quit and primary restoration. It is not a platform IME test.

The independent core candidate had also passed its 192 engine tests (including
seven documentation tests) before promotion. Those are separate from Aether's
127 tests; Aether CI runs its consumer tests, not the entire upstream core suite.

## CI and limits

The new workflow installs official Ubuntu dependencies on ubuntu-24.04, uses a
pinned Rust toolchain and commit-pinned official checkout/setup-node actions,
then runs the consumer tests, strict lint, real Vim PTY smoke and native build.
It grants only contents:read and does not publish artifacts or deploy anything.
The remote workflow run for each exact commit is the CI result; local success
must not be described as GitHub CI success before that run completes.

No native window, WebKit transport, pixels, Wayland, native clipboard or platform
IME acceptance is included. The local launch blocker remains the release WebKit's
compiled absolute subprocess paths, plus an independently observed read-only uid
map failure in a minimal bubblewrap probe. No attempt was made to bypass either.
A normal installed desktop with working sandbox facilities is still required for
GUI acceptance. Mouse encoding and delta budget fixes in the core do not imply
Aether implements mouse forwarding or delta-frame delivery.
