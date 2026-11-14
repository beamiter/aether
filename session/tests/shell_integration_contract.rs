//! Fail-closed consumer baseline, not authenticated shell integration.
//! All OSC arriving on the PTY can be emitted by an arbitrary child process.
use jterm_headless_session_spike::Terminal;

fn feed_chunks(t: &mut Terminal, bytes: &[u8], size: usize) -> Vec<u8> {
    let mut replies = Vec::new();
    for part in bytes.chunks(size) {
        replies.extend(t.feed(part).unwrap());
        assert!(t.feed(b"").unwrap().is_empty());
    }
    replies
}
fn assert_neutral(t: &mut Terminal) {
    assert_eq!(t.visible_text().trim_end(), "SAFE");
    assert_eq!(t.feed(b"\x1b[6n").unwrap(), b"\x1b[1;5R");
    assert_eq!(
        t.encode_key("ArrowUp", false, false, false).unwrap(),
        b"\x1b[A"
    );
    assert_eq!(t.encode_paste("echo safe").unwrap(), b"echo safe");
}

#[test]
fn complete_untrusted_shell_markers_are_inert_at_every_split() {
    // OSC 7 cwd, OSC 133 lifecycle and a private producer record are data only.
    let records: &[&[u8]] = &[
        b"\x1b]7;file://host/tmp/claimed\x07",
        b"\x1b]133;A\x1b\\",
        b"\x1b]133;B\x07",
        b"\x1b]133;C\x07",
        b"\x1b]133;D;0\x07",
        b"\x1b]777;jsh;command=claimed;cwd=/tmp\x07",
    ];
    for bytes in records {
        for split in 0..=bytes.len() {
            let mut t = Terminal::new(3, 40).unwrap();
            t.feed(b"SAFE").unwrap();
            assert!(t.feed(&bytes[..split]).unwrap().is_empty());
            assert!(t.feed(&bytes[split..]).unwrap().is_empty());
            assert_neutral(&mut t);
            assert_eq!(t.unsupported_count(), 1);
        }
    }
}

#[test]
fn cancelled_shell_records_recover_without_swallowing_following_text() {
    for cancel in [0x18, 0x1a] {
        for size in 1..=32 {
            let mut t = Terminal::new(3, 40).unwrap();
            let mut bytes = b"\x1b]133;C;command=forged".to_vec();
            bytes.push(cancel);
            // No fresh ESC before SAFE: proves cancellation itself recovers.
            bytes.extend_from_slice(b"SAFE");
            assert!(feed_chunks(&mut t, &bytes, size).is_empty());
            assert_neutral(&mut t);
        }
    }
}

#[test]
fn oversized_shell_records_recover_at_bel_and_st() {
    for terminator in [b"\x07".as_slice(), b"\x1b\\".as_slice()] {
        for size in [1, 7, 4096, 65536] {
            let mut t = Terminal::new(3, 40).unwrap();
            let mut bytes = b"\x1b]7;file://host/".to_vec();
            bytes.extend(std::iter::repeat_n(b'x', 16384));
            bytes.extend_from_slice(terminator);
            bytes.extend_from_slice(b"SAFE");
            assert!(feed_chunks(&mut t, &bytes, size).is_empty());
            assert_neutral(&mut t);
        }
    }
}

#[test]
fn unterminated_record_does_not_advertise_completion_and_cancellation_recovers() {
    let mut t = Terminal::new(3, 40).unwrap();
    assert!(t.feed(b"\x1b]133;D;0").unwrap().is_empty());
    assert_eq!(t.unsupported_count(), 0); // no complete event yet
    assert!(t.visible_text().trim().is_empty());
    assert!(t.feed(b"\x18SAFE").unwrap().is_empty());
    assert_neutral(&mut t);
}
