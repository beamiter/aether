use super::*;

#[test]
fn oversized_strings_are_partition_independent_and_cannot_execute_nested_controls() {
    for kind in *b"]P_X^" {
        for chunk in [1, 7, 4095, 4096, 4097, 65536] {
            let mut terminal = Terminal::new(2, 16).unwrap();
            terminal.feed(b"SAFE").unwrap();
            let mut input = vec![0x1b, kind];
            input.extend(vec![b'x'; 8192]);
            input.extend(b"\x1b[2J\x1b[6n\x1bP+q544e");
            input.extend(vec![b'z'; 8192]);
            input.extend(b"\x1b\\OK");
            for part in input.chunks(chunk) {
                assert!(terminal.feed(part).unwrap().is_empty());
            }
            assert_eq!(terminal.parser.rejected_count(), 1);
            assert_eq!(terminal.negative_query_replies(), 0);
            assert!(terminal.visible_text().starts_with("SAFEOK"));
            assert_eq!(
                terminal.feed(b"\x1bP+q544e\x1b\\").unwrap(),
                b"\x1bP0+r\x1b\\"
            );
        }
    }
}

#[test]
fn canceled_strings_recover_without_replaying_payload_or_replying() {
    for kind in *b"]P_X^" {
        for cancel in [0x18, 0x1a] {
            for oversize in [false, true] {
                let mut terminal = Terminal::new(2, 16).unwrap();
                let mut input = vec![0x1b, kind];
                input.extend(b"+q544e");
                input.extend(vec![b'x'; if oversize { 8192 } else { 20 }]);
                input.push(cancel);
                input.extend(b"RECOVERED");
                for part in input.chunks(3) {
                    assert!(terminal.feed(part).unwrap().is_empty());
                }
                assert!(terminal.visible_text().starts_with("RECOVERED"));
                assert_eq!(terminal.parser.rejected_count(), u64::from(oversize));
            }
        }
    }
}

#[test]
fn dcs_exact_payload_limit_accepts_but_one_more_rejects_across_feeds() {
    for payload_len in [4095, 4096, 4097] {
        for chunk in [1, 13, 4096, 65536] {
            let mut terminal = Terminal::new(2, 16).unwrap();
            let mut input = b"\x1bP+q".to_vec();
            input.extend(vec![b'a'; payload_len - 2]);
            input.extend(b"\x1b\\");
            let mut replies = Vec::new();
            for part in input.chunks(chunk) {
                replies.extend(terminal.feed(part).unwrap());
            }
            if payload_len <= 4096 {
                assert_eq!(replies, b"\x1bP0+r\x1b\\");
                assert_eq!(terminal.parser.rejected_count(), 0);
            } else {
                assert!(replies.is_empty());
                assert_eq!(terminal.parser.rejected_count(), 1);
            }
        }
    }
}

#[test]
fn repeated_osc_oversize_recovers_on_bel_and_accepts_fresh_query() {
    let mut terminal = Terminal::new(2, 16).unwrap();
    terminal.feed(b"SAFE").unwrap();
    for round in 1..=2 {
        terminal.feed(b"\x1b]0;").unwrap();
        for _ in 0..1024 {
            assert!(terminal.feed(&[b'x'; 1024]).unwrap().is_empty());
        }
        assert_eq!(terminal.parser.rejected_count(), round);
        assert!(terminal.feed(b"\x07OK").unwrap().is_empty());
    }
    assert!(terminal.visible_text().starts_with("SAFEOKOK"));
    assert_eq!(
        terminal.feed(b"\x1bP+q544e\x1b\\").unwrap(),
        b"\x1bP0+r\x1b\\"
    );
}
