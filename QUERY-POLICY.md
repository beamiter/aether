# Query policy checkpoint

This experimental host does not inherit Ember's identity or advertise its unimplemented capabilities.

- Existing shared DECRQSS is reused.
- XTGETTCAP (`DCS +q`) receives the fixed negative response `ESC P 0+r ESC \\`, only for payloads no longer than 4096 bytes. No query payload is reflected and no capability table is advertised. The reply counts both as a negative response and as an unsupported capability request.
- DA1 and DA2 remain unsupported. The shared legacy reducer answers for Ember, but Aether explicitly keeps CSI `c` out of that reducer rather than reporting another terminal's identity. ESC `c` (RIS) is not part of this guard.
- Vim's `DCS zz` is deliberately an unknown-sequence compatibility probe, not a query requiring a reply. It must not move the cursor or print text. The following `CSI 0 % m` must not reset SGR. A local adapter guard fixes that behavior for the pinned shared core; this does not mean the shared core upstream has been patched.
- Unknown DCS/OSC remain opaque. No clipboard/OSC52, filesystem, process execution or network operations are triggered by output.
- Fixed structural diagnostics omit arbitrary DCS/OSC payloads. Temporary probe byte logging was removed.

Sources inspected:
- Vim's `check_terminal_behavior`, version 9.1.1451: https://raw.githubusercontent.com/vim/vim/v9.1.1451/src/term.c (around lines 3947–3966).
- XTerm XTGETTCAP negative-response semantics: https://invisible-mirror.net/xterm/ctlseqs/ctlseqs.html
- Pinned `jterm_core@57ed89a` semantics and the previously recovered Ember/Frost host dispatch implementations.

Tests include actual fixed-shell PTY query/response roundtrip (exact seven reply bytes), repeated-query reply-budget exhaustion, canceled and oversized DCS, fragmentation, and preserving style/cursor through Vim's compatibility probe. Real Vim still passes full editing/UTF-8 smoke; its unsupported count rises from 13 to 14 because the formerly misinterpreted intermediate SGR is now correctly recorded as unsupported.
