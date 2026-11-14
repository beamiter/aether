fn main() {
    let frame = tauri_terminal_frame_bridge::DemoBridge::default()
        .next_frame()
        .expect("valid demo frame");
    println!("{}", serde_json::to_string(&frame).expect("serialize"));
}
