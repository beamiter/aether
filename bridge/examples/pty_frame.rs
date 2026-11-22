//! Fixed-command noninteractive smoke. Does not accept arbitrary command arguments.
use tauri_terminal_frame_bridge::live::LiveBridge;
fn main() -> Result<(), String> {
    let mut b = LiveBridge::default();
    let epoch = b.open(12, 80)?;
    let owner = b.attachment(epoch)?;
    let bytes = b"printf 'RUST_PTY_FRAME_OK_271828\\n'; exit\n";
    let mut at = 0;
    let start = std::time::Instant::now();
    let mut latest = None;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if at < bytes.len() {
            at += b.write_owned_raw(owner.clone(), bytes[at..].to_vec())?;
        }
        if let Some(frame) = b.poll(epoch)? {
            b.acknowledge(epoch, frame.revision)?;
            latest = Some(frame);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    b.close(epoch)?;
    let frame = latest.ok_or("no PTY frame")?;
    if !frame.rows.iter().any(|r| {
        r.iter()
            .map(|c| c.text.as_str())
            .collect::<String>()
            .trim()
            .trim_start_matches("$ ")
            == "RUST_PTY_FRAME_OK_271828"
    }) {
        return Err(format!(
            "fixed PTY output missing: {:?}",
            frame
                .rows
                .iter()
                .map(|r| r.iter().map(|c| c.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
        ));
    }
    println!(
        "{}",
        serde_json::to_string(&frame).map_err(|e| e.to_string())?
    );
    Ok(())
}
