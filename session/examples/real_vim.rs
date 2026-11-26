//! Real Vim smoke: test-only executable path, never exposed through Tauri commands.
use jterm_engine::frame::Screen;
use jterm_headless_session_spike::Session;
use std::{
    num::NonZeroU64,
    path::PathBuf,
    thread::sleep,
    time::{Duration, Instant},
};
fn send(s: &mut Session, bytes: &[u8]) {
    let start = Instant::now();
    let mut at = 0;
    while at < bytes.len() {
        at += s.write(&bytes[at..]).expect("PTY input");
        s.poll().expect("PTY poll");
        assert!(start.elapsed() < Duration::from_secs(10), "input timeout");
        sleep(Duration::from_millis(5));
    }
}
fn until(s: &mut Session, what: &str, predicate: impl Fn(&Session) -> bool) {
    let start = Instant::now();
    loop {
        s.poll().expect("PTY poll");
        if predicate(s) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{what}: {}",
            s.terminal().visible_text()
        );
        sleep(Duration::from_millis(10));
    }
}
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn main() {
    let vim = std::fs::canonicalize(
        std::env::args()
            .nth(1)
            .expect("official Vim executable path"),
    )
    .unwrap();
    let dir: PathBuf = std::env::temp_dir().join(format!("jterm-vim-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("smoke.txt");
    let mut s = Session::spawn_family_with_epoch(24, 80, NonZeroU64::MIN, false).unwrap();
    let cmd = format!(
        "test \"$TERM\" = jterm-256color && test \"$(tput colors)\" = 256 && \
         printf '\\120RIMARY_SENTINEL\\n' && {} -Nu NONE -n -i NONE {}; printf '\\126IM_RETURNED\\n'\n",
        quote(vim.to_str().unwrap()),
        quote(file.to_str().unwrap())
    );
    send(&mut s, cmd.as_bytes());
    until(&mut s, "Vim alternate entry", |s| {
        s.terminal().frame().unwrap().stamp().screen == Screen::Alternate
    });
    send(&mut s, b"i");
    let paste = s.terminal().encode_paste("JTERM_VIM_REAL_271828").unwrap();
    assert!(
        paste.starts_with(b"\x1b[200~"),
        "Vim must request bracketed paste"
    );
    send(&mut s, &paste);
    let escape = s
        .terminal()
        .encode_key("Escape", false, false, false)
        .unwrap();
    send(&mut s, &escape);
    until(&mut s, "Vim insert rendering", |s| {
        s.terminal()
            .visible_text()
            .contains("JTERM_VIM_REAL_271828")
    });
    let left = s
        .terminal()
        .encode_key("ArrowLeft", false, false, false)
        .unwrap();
    assert_eq!(left, b"\x1bOD", "Vim application cursor mode");
    send(&mut s, &left);
    until(&mut s, "application left arrow", |s| {
        s.terminal().frame().unwrap().cursor().col == 19
    });
    let right = s
        .terminal()
        .encode_key("ArrowRight", false, false, false)
        .unwrap();
    send(&mut s, &right);
    until(&mut s, "application right arrow", |s| {
        s.terminal().frame().unwrap().cursor().col == 20
    });
    s.resize(30, 100).unwrap();
    send(&mut s, b"\x0c");
    until(&mut s, "Vim resize rendering", |s| {
        s.terminal()
            .visible_text()
            .contains("JTERM_VIM_REAL_271828")
            && s.terminal().frame().unwrap().cols() == 100
    });
    send(&mut s, b"o");
    let committed = s.terminal().encode_text("中文输入测试").unwrap();
    send(&mut s, &committed);
    send(&mut s, &escape);
    let mut expected = String::from("JTERM_VIM_REAL_271828\n中文输入测试\n");
    for n in 1..=35 {
        let line = format!("VIM_LINE_{n:02}");
        send(&mut s, format!("o{line}\x1b").as_bytes());
        expected.push_str(&line);
        expected.push('\n');
    }
    send(&mut s, b"ggdduG"); // delete first line, undo, return to bottom
    until(&mut s, "Vim scroll and undo", |s| {
        s.terminal().visible_text().contains("VIM_LINE_35")
    });
    send(&mut s, b":wq\r");
    until(&mut s, "primary restored", |s| {
        s.terminal().frame().unwrap().stamp().screen == Screen::Primary
            && s.terminal()
                .visible_text()
                .lines()
                .any(|l| l.trim_end() == "VIM_RETURNED")
    });
    assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
    assert!(s.terminal().visible_text().contains("PRIMARY_SENTINEL"));
    println!("PASS real Vim: alternate entry, insert render, bracketed paste, application arrows, exact Chinese committed text, resize, scroll/edit/undo, write/quit, exact file content, primary restoration; unsupported={}",s.terminal().unsupported_count());
    println!(
        "Unsupported identifiers: {:?}",
        s.terminal().unsupported_events()
    );
    send(&mut s, b"exit\n");
    until(&mut s, "shell drained", Session::is_drained);
    s.close().unwrap();
    std::fs::remove_file(file).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
