//! Aether host contract, through the production parser and archive entry points.
use jterm_headless_session_spike::Terminal;

fn feed_at_cut(t: &mut Terminal, input: &str, cut: usize) -> Vec<u8> {
    let mut output = t.feed(&input.as_bytes()[..cut]).unwrap();
    output.extend(t.feed(&input.as_bytes()[cut..]).unwrap());
    output
}

#[test]
fn copies_only_the_virgin_gap_differently_without_changing_live_cells() {
    let mut baseline = Terminal::new(2, 4).unwrap();
    baseline.feed("abc 界D".as_bytes()).unwrap();
    let baseline_frame = baseline.frame().unwrap();
    let inputs = [
        ("abc界D", "abc界D", ""),
        ("abc 界D", "abc 界D", " "),
        ("abc \x1b[1;4H界D", "abc 界D", " "),
    ];
    for (input, expected, margin) in inputs {
        for cut in 0..=input.len() {
            let mut t = Terminal::new(2, 4).unwrap();
            assert!(feed_at_cut(&mut t, input, cut).is_empty());
            let frame = t.frame().unwrap();
            assert_eq!(frame.cursor(), baseline_frame.cursor());
            for (row, baseline) in frame.rows().iter().zip(baseline_frame.rows()) {
                assert_eq!(row.cells, baseline.cells);
                assert_eq!(row.wrapped, baseline.wrapped);
            }
            assert_eq!(t.history_rows(), 0);
            t.feed(b"\r\n\r\n").unwrap();
            let page = t.history_page(0, 2).unwrap();
            assert_eq!(page.row_count, 2);
            assert_eq!(page.text, expected, "{input:?} cut={cut}");
            assert_eq!(page.selection_text(0, 3, 0, 4).unwrap(), margin);
            assert_eq!(page.selection_text(1, 1, 1, 2).unwrap(), "");
            assert!(page.selection_text(0, 3, 0, 5).is_err());
        }
    }
}

#[test]
fn old_archived_spaces_and_owned_page_coordinates_survive_new_output() {
    // Start with an already archived, deliberately authored margin, then
    // produce a distinct synthetic gap without changing old page ownership.
    let mut t = Terminal::new(2, 4).unwrap();
    t.feed("old 界A\r\n\r\n".as_bytes()).unwrap();
    let old = t.history_page(0, 2).unwrap();
    assert_eq!(old.text, "old 界A");
    t.feed("new界B\r\n\r\n".as_bytes()).unwrap();
    let new = t.history_page(0, 2).unwrap();
    assert_eq!(new.text, "new界B");
    let offset = new.total_rows - old.total_rows;
    assert_eq!(t.history_page(offset, 2).unwrap().text, "old 界A");
    t.resize(3, 7).unwrap();
    t.feed(b"\x1b[?1049h").unwrap();
    assert!(t.history_page(0, 2).unwrap().text.is_empty());
    t.feed(b"\x1b[?1049l\x1b[3J").unwrap();
    assert_eq!(t.history_rows(), 0);
    assert_eq!(old.text, "old 界A");
    assert_eq!(old.selection_text(0, 3, 0, 4).unwrap(), " ");
    assert_eq!(new.selection_text(0, 3, 0, 4).unwrap(), "");
}

#[test]
fn edited_and_styled_margin_spaces_still_copy_as_literal_spaces() {
    for input in [
        "abc\x1b[41m \x1b[0m界D",
        "abc \x1b[1;4H界D",
        "abc界D\x1b[1;4H\x1b[X",
    ] {
        let mut t = Terminal::new(2, 4).unwrap();
        t.feed(input.as_bytes()).unwrap();
        t.feed(b"\x1b[2;4H\r\n\r\n").unwrap();
        let page = t.history_page(0, 2).unwrap();
        assert_eq!(page.text, "abc 界D", "{input:?}");
        assert_eq!(page.selection_text(0, 3, 0, 4).unwrap(), " ");
    }
}

#[test]
fn engine_repin_does_not_inherit_another_terminals_da_identity() {
    let input = "\x1b[c\x1b[>0c\x1b[?0c\x1b[0$c\x1b[5n";
    for cut in 0..=input.len() {
        let mut t = Terminal::new(2, 8).unwrap();
        t.feed(b"persist").unwrap();
        assert_eq!(feed_at_cut(&mut t, input, cut), b"\x1b[0n");
        assert!(t.visible_text().starts_with("persist"));
        assert_eq!(t.unsupported_count(), 4);
        assert_eq!(t.unsupported_events().len(), 4);
        for (event, prefix) in t.unsupported_events().iter().zip([
            "CSI prefix=None ",
            "CSI prefix=Some(62) ",
            "CSI prefix=Some(63) ",
            "CSI prefix=None ",
        ]) {
            assert!(event.starts_with(prefix), "{event}");
            assert!(event.contains(" final=99 "), "{event}");
        }
    }
}

#[test]
fn ris_resets_screen_and_modes_without_advertising_device_identity() {
    let ris = "\x1bc";
    for cut in 0..=ris.len() {
        let mut t = Terminal::new(2, 8).unwrap();
        let initial = t.frame().unwrap();
        t.feed(b"archived\r\n\r\npersist\x1b[?2004h").unwrap();
        assert!(t.visible_text().contains("persist"));
        assert!(t.history_rows() > 0);
        assert_ne!(t.frame().unwrap().cursor(), initial.cursor());
        assert_eq!(t.feed(b"\x1b[?2004$p").unwrap(), b"\x1b[?2004;1$y");

        assert!(feed_at_cut(&mut t, ris, cut).is_empty());
        assert_eq!(t.unsupported_count(), 0);
        assert!(t.unsupported_events().is_empty());
        assert_eq!(t.history_rows(), 0);
        let reset = t.frame().unwrap();
        assert_eq!(reset.cursor(), initial.cursor());
        for (row, initial) in reset.rows().iter().zip(initial.rows()) {
            assert_eq!(row.cells, initial.cells);
            assert_eq!(row.wrapped, initial.wrapped);
        }
        assert_eq!(t.feed(b"\x1b[?2004$p").unwrap(), b"\x1b[?2004;2$y");
    }
}
