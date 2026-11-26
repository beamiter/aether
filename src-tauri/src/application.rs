#[path = "agent_workspace.rs"]
mod agent_workspace;

pub(crate) use agent_workspace::Workspace;
use std::sync::{Arc, Mutex};
use tauri_terminal_frame_bridge::{
    live::{LiveBridge, MouseInput, SessionStatus},
    DemoBridge, WireFrame,
};
#[tauri::command]
async fn demo_frame(bridge: tauri::State<'_, Arc<Mutex<DemoBridge>>>) -> Result<WireFrame, String> {
    run_bridge(bridge.inner().clone(), move |bridge| bridge.next_frame()).await
}
#[tauri::command]
async fn acknowledge(
    session: String,
    epoch: u32,
    revision: u32,
    bridge: tauri::State<'_, Arc<Mutex<DemoBridge>>>,
) -> Result<(), String> {
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.acknowledge(&session, epoch, revision)
    })
    .await
}
#[tauri::command]
async fn open_shell(
    rows: Option<usize>,
    cols: Option<usize>,
    bridge: tauri::State<'_, Arc<Mutex<LiveBridge>>>,
) -> Result<u32, String> {
    let (rows, cols) = match (rows, cols) {
        (Some(rows), Some(cols)) => (rows, cols),
        (None, None) => (24, 80),
        _ => return Err("opening with a custom viewport requires both rows and columns".into()),
    };
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.open(rows, cols)
    })
    .await
}
#[tauri::command]
async fn poll_shell(
    epoch: u32,
    bridge: tauri::State<'_, Arc<Mutex<LiveBridge>>>,
) -> Result<Option<WireFrame>, String> {
    run_bridge(bridge.inner().clone(), move |bridge| bridge.poll(epoch)).await
}
#[tauri::command]
async fn shell_status(
    epoch: u32,
    bridge: tauri::State<'_, Arc<Mutex<LiveBridge>>>,
) -> Result<SessionStatus, String> {
    run_bridge(bridge.inner().clone(), move |bridge| bridge.status(epoch)).await
}
#[tauri::command]
async fn ack_shell(
    epoch: u32,
    revision: u32,
    bridge: tauri::State<'_, Arc<Mutex<LiveBridge>>>,
) -> Result<(), String> {
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.acknowledge(epoch, revision)
    })
    .await
}
#[tauri::command]
async fn write_shell(
    epoch: u32,
    bytes: Vec<u8>,
    bridge: tauri::State<'_, Arc<Mutex<LiveBridge>>>,
) -> Result<usize, String> {
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.write(epoch, &bytes)
    })
    .await
}
#[tauri::command]
async fn resize_shell(
    epoch: u32,
    rows: usize,
    cols: usize,
    bridge: tauri::State<'_, Arc<Mutex<LiveBridge>>>,
) -> Result<(), String> {
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.resize(epoch, rows, cols)
    })
    .await
}
#[tauri::command]
async fn close_shell(
    epoch: u32,
    bridge: tauri::State<'_, Arc<Mutex<LiveBridge>>>,
) -> Result<tauri_terminal_frame_bridge::live::CloseStatus, String> {
    run_bridge(bridge.inner().clone(), move |bridge| bridge.close(epoch)).await
}
#[tauri::command]
async fn key_shell(
    epoch: u32,
    name: String,
    shift: bool,
    alt: bool,
    ctrl: bool,
    bridge: tauri::State<'_, Arc<Mutex<LiveBridge>>>,
) -> Result<(), String> {
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.key(epoch, &name, shift, alt, ctrl)
    })
    .await
}
#[tauri::command]
async fn mouse_shell(
    epoch: u32,
    report: MouseInput,
    bridge: tauri::State<'_, Arc<Mutex<LiveBridge>>>,
) -> Result<bool, String> {
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.mouse(epoch, report)
    })
    .await
}
#[tauri::command]
async fn paste_shell(
    epoch: u32,
    text: String,
    bridge: tauri::State<'_, Arc<Mutex<LiveBridge>>>,
) -> Result<(), String> {
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.paste(epoch, &text)
    })
    .await
}
#[tauri::command]
async fn text_shell(
    epoch: u32,
    text: String,
    bridge: tauri::State<'_, Arc<Mutex<LiveBridge>>>,
) -> Result<(), String> {
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.text(epoch, &text)
    })
    .await
}
#[tauri::command]
async fn history_shell(
    epoch: u32,
    offset: usize,
    bridge: tauri::State<'_, Arc<Mutex<LiveBridge>>>,
) -> Result<tauri_terminal_frame_bridge::live::HistorySnapshot, String> {
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.history(epoch, offset)
    })
    .await
}
#[tauri::command]
async fn recover_shell(
    epoch: u32,
    bridge: tauri::State<'_, Arc<Mutex<LiveBridge>>>,
) -> Result<WireFrame, String> {
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.recover_frame(epoch)
    })
    .await
}
// PTY creation, parsing and mutex contention run on the blocking pool, never
// on the window event thread or an async executor worker. No lock crosses await.
async fn run_bridge<B, T>(
    bridge: Arc<Mutex<B>>,
    operation: impl FnOnce(&mut B) -> Result<T, String> + Send + 'static,
) -> Result<T, String>
where
    B: Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || {
        let mut bridge = bridge.lock().map_err(|_| "session unavailable")?;
        operation(&mut bridge)
    })
    .await
    .map_err(|_| "session worker unavailable".to_string())?
}
pub(crate) fn configure<R: tauri::Runtime>(
    builder: tauri::Builder<R>,
    workspace: Workspace,
) -> tauri::Builder<R> {
    builder
        .manage(Arc::new(Mutex::new(DemoBridge::default())))
        .manage(Arc::new(Mutex::new(LiveBridge::default())))
        .manage(workspace)
        .invoke_handler(tauri::generate_handler![
            agent_workspace::workspace_refresh,
            agent_workspace::workspace_catalog,
            agent_workspace::workspace_search,
            agent_workspace::workspace_read_document,
            agent_workspace::workspace_prepare_cli_delete,
            agent_workspace::workspace_delete_cli_session,
            demo_frame,
            acknowledge,
            open_shell,
            poll_shell,
            shell_status,
            history_shell,
            ack_shell,
            recover_shell,
            write_shell,
            resize_shell,
            key_shell,
            mouse_shell,
            text_shell,
            paste_shell,
            close_shell
        ])
}
