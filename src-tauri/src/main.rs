use local_sessions::LocalSessions;
use std::sync::{Arc, Mutex};
use tauri_terminal_frame_bridge::{
    live::{LiveBridge, SessionStatus},
    semantic::{Attachment, Resume, SessionSelector, Update},
    DemoBridge, WireFrame,
};

#[path = "../vendor/jwm-remote/client.rs"]
#[allow(
    dead_code,
    reason = "Other frontends use the shared client's native clipboard controls."
)]
mod jwm_remote_session;
mod local_sessions;
mod remote_desktop;
mod ssh_terminal;
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
async fn open_shell<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<Attachment, String> {
    require_terminal_window(&window)?;
    run_bridge(bridge.inner().clone(), LocalSessions::open).await
}

#[tauri::command]
async fn attach_shell<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    target: Option<SessionSelector>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<Option<Resume>, String> {
    require_terminal_window(&window)?;
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.attach(target.as_ref())
    })
    .await
}

#[tauri::command]
async fn detach_shell(
    owner: Attachment,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    run_bridge(bridge.inner().clone(), move |bridge| bridge.detach(&owner)).await
}

#[tauri::command]
async fn poll_shell(
    owner: Attachment,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<Option<Update>, String> {
    run_attached(bridge.inner().clone(), owner, LiveBridge::poll_update).await
}
#[tauri::command]
async fn shell_status(
    owner: Attachment,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<SessionStatus, String> {
    run_attached(bridge.inner().clone(), owner, |bridge, epoch| {
        bridge.status(epoch)
    })
    .await
}
#[tauri::command]
async fn ack_shell(
    owner: Attachment,
    revision: u32,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    run_attached(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.acknowledge(epoch, revision)
    })
    .await
}
#[tauri::command]
async fn write_shell(
    owner: Attachment,
    bytes: Vec<u8>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<usize, String> {
    run_input(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.write(epoch, &bytes)
    })
    .await
}
#[tauri::command]
async fn resize_shell(
    owner: Attachment,
    rows: usize,
    cols: usize,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    run_input(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.resize(epoch, rows, cols)
    })
    .await
}
#[tauri::command]
async fn close_shell(
    owner: Attachment,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<tauri_terminal_frame_bridge::live::CloseStatus, String> {
    run_bridge(bridge.inner().clone(), move |bridge| bridge.close(&owner)).await
}
#[tauri::command]
async fn key_shell(
    owner: Attachment,
    name: String,
    shift: bool,
    alt: bool,
    ctrl: bool,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    run_input(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.key(epoch, &name, shift, alt, ctrl)
    })
    .await
}
#[tauri::command]
async fn paste_shell(
    owner: Attachment,
    text: String,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    run_input(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.paste(epoch, &text)
    })
    .await
}
#[tauri::command]
async fn text_shell(
    owner: Attachment,
    text: String,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    run_input(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.text(epoch, &text)
    })
    .await
}
#[tauri::command]
async fn history_shell(
    owner: Attachment,
    offset: usize,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<tauri_terminal_frame_bridge::live::HistorySnapshot, String> {
    run_attached(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.history(epoch, offset)
    })
    .await
}
#[tauri::command]
async fn recover_shell(
    owner: Attachment,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<Resume, String> {
    run_bridge(bridge.inner().clone(), move |bridge| bridge.recover(&owner)).await
}

fn require_terminal_window<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("The terminal belongs to the main application window.".into())
    }
}

async fn run_attached<T: Send + 'static>(
    bridge: Arc<Mutex<LocalSessions>>,
    owner: Attachment,
    operation: impl FnOnce(&mut LiveBridge, u32) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    run_bridge(bridge, move |bridge| bridge.attached(&owner, operation)).await
}

async fn run_input<T: Send + 'static>(
    bridge: Arc<Mutex<LocalSessions>>,
    owner: Attachment,
    operation: impl FnOnce(&mut LiveBridge, u32) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    run_bridge(bridge, move |bridge| bridge.input(&owner, operation)).await
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
fn application<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
        .manage(Arc::new(Mutex::new(DemoBridge::default())))
        .manage(Arc::new(Mutex::new(LocalSessions::default())))
        .manage(Arc::new(Mutex::new(ssh_terminal::SshTerminal::default())))
        .manage(Arc::new(
            Mutex::new(remote_desktop::WindowRemote::default()),
        ))
        .on_window_event(remote_desktop::window_event)
        .invoke_handler(tauri::generate_handler![
            demo_frame,
            acknowledge,
            open_shell,
            attach_shell,
            detach_shell,
            poll_shell,
            shell_status,
            history_shell,
            ack_shell,
            recover_shell,
            write_shell,
            resize_shell,
            key_shell,
            text_shell,
            paste_shell,
            close_shell,
            ssh_terminal::ssh_terminal,
            remote_desktop::remote_desktop
        ])
}

fn main() {
    application(tauri::Builder::default())
        .run(tauri::generate_context!())
        .expect("Tauri runtime");
}

#[cfg(test)]
mod ipc_tests;
