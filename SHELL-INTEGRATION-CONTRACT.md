# Shell integration consumer baseline (2026-11-13)

This is a deliberately fail-closed fallback contract for the fifth terminal, not a command-block feature or authenticated shell integration.

The PTY byte stream does not authenticate the source of OSC 7, OSC 133 or private metadata. This prototype currently ignores completed shell metadata as unsupported. It does not expose cwd, command IDs, trust decisions, or UI actions from these records. Do not reinterpret a successful test as trusted shell provenance.

`session/tests/shell_integration_contract.rs` contains four independent, hand-written fixtures:

- OSC 7, OSC 133 A/B/C/D and a representative private record remain inert at every two-part split.
- CAN/SUB cancellation recovers at chunk sizes 1 through 32, with a printable sentinel *before* any new escape sequence.
- Oversized records recover at BEL and ST terminators for single-byte, small, boundary and whole-feed delivery.
- Unterminated records do not emit a complete unsupported event, and cancellation recovers.

The assertions check visible text, cursor-report bytes, arrow-key encoding, paste encoding and lack of unsolicited replies. They do not prove all hidden state is unchanged or authenticate any metadata. Future command-block support must replace this fallback deliberately with a versioned trust/provenance contract, not promote arbitrary PTY bytes into trusted actions.

Executed: 4 new tests, full session suite (51 unit + 2 query-limit + 4 contract tests), and strict all-targets Clippy pass. This run did not re-run bridge/Node/native/Vim tests; v13 evidence remains unchanged. No production code or lock file changed.

Environment discovery found only this Dot cloud computer. A fresh AF_UNIX socket probe still fails with EPERM. No safe alternative connected GUI host was available, no system security setting was changed, and no unauthorized repository or remote runner was used. GUI/IME/pixel acceptance remains blocked.
