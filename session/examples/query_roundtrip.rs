//! Fixed shell PTY query roundtrip, no arbitrary payload supplied by external sources.
use jterm_headless_session_spike::Session;
use std::{
    thread::sleep,
    time::{Duration, Instant},
};
fn main() {
    let mut s = Session::spawn(12, 100).unwrap();
    let command=b"stty -echo -icanon min 0 time 10; printf '\\033P+q544e\\033\\\\'; reply=$(dd bs=1 count=7 2>/dev/null | od -An -tx1 | tr -d ' \\n'); stty sane; printf '\\nQUERY_REPLY_%s\\n' \"$reply\"; exit\n";
    let mut sent = 0;
    let start = Instant::now();
    while !s.is_drained() {
        if sent < command.len() {
            sent += s.write(&command[sent..]).unwrap();
        }
        s.poll().unwrap();
        assert!(start.elapsed() < Duration::from_secs(10), "query timeout");
        sleep(Duration::from_millis(5));
    }
    let text = s.terminal().visible_text();
    assert!(
        text.lines()
            .any(|l| l.trim_end() == "QUERY_REPLY_1b50302b721b5c"),
        "{text}"
    );
    assert_eq!(s.terminal().negative_query_replies(), 1);
    assert_eq!(s.dropped_reply_bytes(), 0);
    assert!(s.write_error().is_none());
    s.close().unwrap();
    println!(
        "PASS real PTY XTGETTCAP: exact fixed negative reply, one response, no dropped reply bytes"
    );
}
