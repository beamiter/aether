//! Actual Rust VT state serialized into the Canvas contract (not a screenshot).
fn main() -> Result<(), String> {
    let mut terminal = jterm_headless_session_spike::Terminal::new(4, 24)?;
    terminal.feed(
        "\x1b[38;2;18;52;86;48;5;196;1;3;4;9m中文\x1b[0m A\r\n\x1b[7;31;44m逆\x1b[0m \x1b[2mDim"
            .as_bytes(),
    )?;
    let wire = tauri_terminal_frame_bridge::from_engine(&terminal.frame()?, "unicode-demo", 1)?;
    println!(
        "{}",
        serde_json::to_string(&wire).map_err(|e| e.to_string())?
    );
    Ok(())
}
