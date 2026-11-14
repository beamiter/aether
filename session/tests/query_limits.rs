use jterm_headless_session_spike::Terminal;
#[test]
fn repeated_queries_cannot_exceed_one_feed_reply_budget() {
    let mut t = Terminal::new(2, 10).unwrap();
    let input = b"\x1bP+q\x1b\\".repeat(10_000);
    assert!(input.len() <= 65_536);
    assert_eq!(t.feed(&input).unwrap_err(), "reply limit");
}
#[test]
fn canceled_and_oversized_dcs_do_not_emit_capability_replies() {
    for cancel in [0x18, 0x1a] {
        let mut t = Terminal::new(2, 10).unwrap();
        let mut input = b"\x1bP+q544e".to_vec();
        input.push(cancel);
        assert!(t.feed(&input).unwrap().is_empty());
        assert_eq!(t.negative_query_replies(), 0);
    }
    let mut t = Terminal::new(2, 10).unwrap();
    let mut input = b"\x1bP+q".to_vec();
    input.extend(vec![b'a'; 5000]);
    input.extend(b"\x1b\\");
    assert!(t.feed(&input).unwrap().is_empty());
    assert_eq!(t.negative_query_replies(), 0);
}
