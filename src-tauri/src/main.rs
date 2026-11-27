use local_sessions::{HistoryRequest, LocalSessions, LocalShellStatus, TabActivation};
use std::sync::{Arc, Mutex};
use tauri::{ipc::Channel, webview::PageLoadEvent, Manager};
use tauri_terminal_frame_bridge::{
    history_search::{HistorySearchRequest, HistorySearchResponse},
    live::{LiveBridge, ViewportPixels},
    semantic::{Attachment, Resume, SessionSelector, Update},
    DemoBridge, LocalSnapshot, SnapshotFormat, SnapshotRevision, WireFrame,
};

use jterm_remote_client as jwm_remote_session;
mod agent_workspace;
#[cfg(target_os = "linux")]
mod companion;
mod local_block_search;
mod local_sessions;
mod local_workflows;
mod notebook_panel;
mod output_review;
mod remote_desktop;
mod ssh_files;
mod ssh_terminal;
mod vim9_workbench;
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
    integrated: Option<bool>,
    pane_history_id: Option<String>,
    frame_version: Option<u8>,
    window_pixels: Option<ViewportPixels>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<local_sessions::OpenedShell, String> {
    require_terminal_window(&window)?;
    let format = SnapshotFormat::negotiate(frame_version)?;
    let integrated = integrated.unwrap_or(false);
    let history = HistoryRequest::new(
        integrated,
        pane_history_id,
        window.path().app_data_dir().ok(),
    )?;
    let shared = bridge.inner().clone();
    let weak = Arc::downgrade(&shared);
    run_bridge(shared, move |bridge| {
        bridge.open(integrated, format, history, window_pixels, weak)
    })
    .await
}

#[tauri::command]
async fn open_second_local_shell<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    epoch: u32,
    frame_version: Option<u8>,
    window_pixels: Option<ViewportPixels>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<Attachment, String> {
    require_terminal_window(&window)?;
    let format = SnapshotFormat::negotiate(frame_version)?;
    let shared = bridge.inner().clone();
    let weak = Arc::downgrade(&shared);
    run_bridge(shared, move |bridge| {
        bridge.open_second(epoch, format, window_pixels, weak)
    })
    .await
}

#[tauri::command]
async fn list_local_shells<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<Vec<u32>, String> {
    require_terminal_window(&window)?;
    run_bridge(bridge.inner().clone(), |bridge| Ok(bridge.list_epochs())).await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri binds the top-level IPC fields to separate arguments.
async fn activate_local_tab<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Option<Attachment>,
    target_epoch: Option<u32>,
    preserve_input: bool,
    integrated: Option<bool>,
    pane_history_id: Option<String>,
    frame_version: Option<u8>,
    window_pixels: Option<ViewportPixels>,
) -> Result<Result<local_sessions::AttachedView, String>, String> {
    require_terminal_window(&window)?;
    let format = SnapshotFormat::negotiate(frame_version)?;
    let integrated = integrated.unwrap_or(false);
    let history = HistoryRequest::new(
        integrated,
        pane_history_id,
        window.path().app_data_dir().ok(),
    )?;
    let shared = window
        .try_state::<Arc<Mutex<LocalSessions>>>()
        .ok_or_else(|| "local shell state is unavailable".to_owned())?
        .inner()
        .clone();
    let weak = Arc::downgrade(&shared);
    run_bridge(shared, move |bridge| {
        Ok(bridge.activate_tab(
            owner.as_ref(),
            target_epoch,
            TabActivation {
                preserve_input,
                integrated,
                format,
                history,
                window_pixels,
            },
            weak,
        ))
    })
    .await
}

#[tauri::command]
async fn attach_selected_local_shell<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    epoch: u32,
    frame_version: Option<u8>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<Option<Resume<LocalSnapshot>>, String> {
    require_terminal_window(&window)?;
    let format = SnapshotFormat::negotiate(frame_version)?;
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.attach_selected(epoch, format)
    })
    .await
}

#[tauri::command]
async fn attach_shell<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    target: Option<SessionSelector>,
    frame_version: Option<u8>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<Option<Resume<LocalSnapshot>>, String> {
    require_terminal_window(&window)?;
    let format = SnapshotFormat::negotiate(frame_version)?;
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.attach(target.as_ref(), format)
    })
    .await
}

#[tauri::command]
async fn detach_shell(
    owner: Attachment,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    let shared = bridge.inner().clone();
    let weak = Arc::downgrade(&shared);
    run_bridge(shared, move |bridge| bridge.detach(&owner, weak)).await
}

#[tauri::command]
async fn poll_shell(
    owner: Attachment,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<Option<Update>, String> {
    run_bridge(bridge.inner().clone(), move |bridge| bridge.poll(&owner)).await
}

#[tauri::command]
async fn subscribe_local_shell<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    stream: Channel<local_sessions::StreamEvent>,
    observer: Option<local_sessions::ObserverTarget>,
    frame_version: u8,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    require_terminal_window(&window)?;
    let format = SnapshotFormat::negotiate(Some(frame_version))?;
    if format.channel_version().is_none() {
        return Err("unsupported terminal Channel version".into());
    }
    let shared = bridge.inner().clone();
    let weak = Arc::downgrade(&shared);
    run_bridge(shared, move |bridge| {
        bridge.subscribe(owner, stream, observer, format, weak)
    })
    .await
}

#[tauri::command]
async fn observe_local_shell<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    epoch: u32,
    revision: Option<u32>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<Option<WireFrame>, String> {
    require_terminal_window(&window)?;
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.observe_shell(&owner, epoch, revision)
    })
    .await
}

#[tauri::command]
async fn set_local_observer<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    stream_id: u32,
    observer: Option<local_sessions::ObserverTarget>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    require_terminal_window(&window)?;
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.set_observers(&owner, stream_id, observer.as_slice())
    })
    .await
}

#[tauri::command]
async fn set_local_observers<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    stream_id: u32,
    observers: [Option<local_sessions::ObserverTarget>; 2],
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    require_terminal_window(&window)?;
    run_bridge(bridge.inner().clone(), move |bridge| {
        let observers = observers.into_iter().flatten().collect::<Vec<_>>();
        bridge.set_observers(&owner, stream_id, &observers)
    })
    .await
}

#[tauri::command]
async fn ack_local_stream<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    stream_id: u32,
    sequence: local_sessions::Counter,
    revision: Option<local_sessions::Counter>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    require_terminal_window(&window)?;
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.acknowledge_stream(&owner, stream_id, sequence, revision)
    })
    .await
}

#[tauri::command]
async fn open_terminal_hyperlink<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    revision: local_sessions::Counter,
    row: u16,
    col: u16,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    require_terminal_window(&window)?;
    let target = run_attached(bridge.inner().clone(), owner, move |bridge, _| {
        let target =
            bridge.acknowledged_hyperlink(revision.get(), usize::from(row), usize::from(col))?;
        if !jterm_core::link::is_openable_url(target) {
            return Err("terminal hyperlink is not allowed".into());
        }
        Ok(target.to_owned())
    })
    .await?;
    tauri::async_runtime::spawn_blocking(move || {
        gtk::gio::AppInfo::launch_default_for_uri(&target, None::<&gtk::gio::AppLaunchContext>)
            .map_err(|_| "could not open terminal hyperlink".to_owned())
    })
    .await
    .map_err(|_| "terminal hyperlink opener unavailable".to_owned())?
}

#[tauri::command]
async fn copy_terminal_text<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    text: String,
) -> Result<(), String> {
    use gtk::prelude::WidgetExt;

    require_terminal_window(&window)?;
    if text.is_empty() || text.len() > 1024 * 1024 || text.contains('\0') {
        return Err("Clipboard text must be between 1 byte and 1 MiB without NUL.".into());
    }

    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let owner = window.clone();
    window
        .run_on_main_thread(move || {
            let result = owner
                .gtk_window()
                .map_err(|error| error.to_string())
                .and_then(|native| {
                    let display = WidgetExt::display(&native);
                    let clipboard = gtk::Clipboard::default(&display)
                        .ok_or_else(|| "The system clipboard is unavailable.".to_owned())?;
                    // GTK's generic setter converts newlines for the Wayland
                    // plain-text MIME targets. Publish the originally selected
                    // bytes under each UTF-8 target instead.
                    let targets = ["UTF8_STRING", "text/plain;charset=utf-8", "text/plain"]
                        .map(|mime| gtk::TargetEntry::new(mime, gtk::TargetFlags::empty(), 0));
                    if clipboard.set_with_data(&targets, move |_, data, _| {
                        data.set(&data.target(), 8, text.as_bytes());
                    }) {
                        Ok(())
                    } else {
                        Err("The system clipboard refused the selection.".into())
                    }
                });
            let _ = sender.try_send(result);
        })
        .map_err(|error| error.to_string())?;
    receiver
        .recv()
        .await
        .ok_or_else(|| "The system clipboard is unavailable.".to_owned())?
}

#[tauri::command]
async fn shell_status(
    owner: Attachment,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<LocalShellStatus, String> {
    run_attached(bridge.inner().clone(), owner, |bridge, epoch| {
        LocalShellStatus::read(bridge, epoch)
    })
    .await
}
#[tauri::command]
async fn ack_shell(
    owner: Attachment,
    revision: SnapshotRevision,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.acknowledge_frame(&owner, revision.get())
    })
    .await
}
#[tauri::command]
async fn write_shell(
    owner: Attachment,
    bytes: Vec<u8>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<usize, String> {
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.write_raw(owner, bytes)
    })
    .await
}
#[tauri::command]
async fn resize_shell(
    owner: Attachment,
    rows: usize,
    cols: usize,
    window_pixels: Option<ViewportPixels>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    run_input(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.resize_with_window_pixels(epoch, rows, cols, window_pixels)
    })
    .await
}

#[tauri::command]
async fn set_shell_window_pixels<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    layout: local_sessions::Counter,
    rows: usize,
    cols: usize,
    pixels: ViewportPixels,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    require_terminal_window(&window)?;
    run_input(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.set_window_pixels(epoch, layout.get(), rows, cols, pixels)
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
async fn focus_shell<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    focused: bool,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<bool, String> {
    require_terminal_window(&window)?;
    run_input(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.focus(epoch, focused)
    })
    .await
}
#[tauri::command]
async fn mouse_shell<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    report: tauri_terminal_frame_bridge::live::MouseReport,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<bool, String> {
    require_terminal_window(&window)?;
    run_input(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.mouse(epoch, report)
    })
    .await
}
#[tauri::command]
async fn submit_local_chat<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    prompt: local_sessions::Counter,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<bool, String> {
    require_terminal_window(&window)?;
    run_input(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.submit_prompt_enter(epoch, prompt)
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
async fn history_shell<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    offset: usize,
    frame_version: Option<u8>,
    anchor: Option<tauri_terminal_frame_bridge::protocol_v4::HistorySource>,
    shift: Option<i32>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<tauri_terminal_frame_bridge::protocol_v4::LocalHistorySnapshot, String> {
    use tauri_terminal_frame_bridge::protocol_v4::LocalHistorySnapshot;
    let format = SnapshotFormat::negotiate(frame_version)?;
    if let Some(version) = format.history_version() {
        require_terminal_window(&window)?;
        return run_attached(bridge.inner().clone(), owner, move |bridge, epoch| {
            bridge
                .history_projection_at_version(
                    epoch,
                    offset,
                    anchor,
                    shift.unwrap_or_default(),
                    version,
                )
                .map(LocalHistorySnapshot::Projection)
        })
        .await;
    }
    if anchor.is_some() || shift.is_some() {
        return Err("history anchors require the history projection".into());
    }
    run_attached(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge
            .history(epoch, offset)
            .map(LocalHistorySnapshot::Compatibility)
    })
    .await
}

#[tauri::command]
async fn local_command_blocks<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<tauri_terminal_frame_bridge::live::CommandBlockSnapshot, String> {
    require_terminal_window(&window)?;
    run_attached(bridge.inner().clone(), owner, |bridge, epoch| {
        bridge.command_blocks(epoch)
    })
    .await
}

#[tauri::command]
async fn set_local_command_bookmark<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    selection: tauri_terminal_frame_bridge::live::BlockSelection,
    bookmarked: bool,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<(), String> {
    require_terminal_window(&window)?;
    run_attached(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.set_command_bookmark(epoch, selection, bookmarked)
    })
    .await
}

#[tauri::command]
async fn search_local_history<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    search: HistorySearchRequest,
    frame_version: Option<u8>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<HistorySearchResponse, String> {
    require_terminal_window(&window)?;
    let format = SnapshotFormat::negotiate(frame_version)?;
    run_attached(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.search_history(epoch, search, format)
    })
    .await
}

#[tauri::command]
async fn recall_local_command<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    owner: Attachment,
    selection: tauri_terminal_frame_bridge::live::BlockSelection,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<Option<&'static str>, String> {
    require_terminal_window(&window)?;
    run_input(bridge.inner().clone(), owner, move |bridge, epoch| {
        bridge.recall_command(epoch, &selection)
    })
    .await
}
#[tauri::command]
async fn recover_shell(
    owner: Attachment,
    frame_version: Option<u8>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<local_sessions::AttachedView, String> {
    let format = SnapshotFormat::negotiate(frame_version)?;
    run_bridge(bridge.inner().clone(), move |bridge| {
        bridge.recover(&owner, format)
    })
    .await
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
        .manage(agent_workspace::Workspace::default())
        .manage(Arc::new(Mutex::new(DemoBridge::default())))
        .manage(Arc::new(Mutex::new(LocalSessions::default())))
        .manage(Arc::new(Mutex::new(
            local_workflows::LocalWorkflows::default(),
        )))
        .manage(Arc::new(Mutex::new(
            notebook_panel::NotebookPanel::default(),
        )))
        .manage(Arc::new(
            Mutex::new(output_review::OutputReviews::default()),
        ))
        .manage(Arc::new(Mutex::new(vim9_workbench::Workbench::default())))
        .manage(Arc::new(Mutex::new(ssh_terminal::SshTerminal::default())))
        .manage(Arc::new(Mutex::new(ssh_files::Files::default())))
        .manage(Arc::new(
            Mutex::new(remote_desktop::WindowRemote::default()),
        ))
        .on_page_load(|webview, payload| {
            if webview.label() != "main" || payload.event() != PageLoadEvent::Started {
                return;
            }
            let sessions = webview.state::<Arc<Mutex<LocalSessions>>>();
            let Ok(mut sessions) = sessions.lock() else {
                return;
            };
            if let Err(error) = sessions.retire_renderer() {
                eprintln!("Cannot detach the previous local terminal view: {error}");
            }
        })
        .on_window_event(|window, event| {
            #[cfg(target_os = "linux")]
            if companion::window_event(window, event) {
                return;
            }
            remote_desktop::window_event(window, event);
        })
        .invoke_handler(tauri::generate_handler![
            demo_frame,
            acknowledge,
            open_shell,
            open_second_local_shell,
            list_local_shells,
            activate_local_tab,
            attach_selected_local_shell,
            attach_shell,
            detach_shell,
            poll_shell,
            subscribe_local_shell,
            observe_local_shell,
            set_local_observer,
            set_local_observers,
            ack_local_stream,
            open_terminal_hyperlink,
            copy_terminal_text,
            shell_status,
            history_shell,
            local_command_blocks,
            set_local_command_bookmark,
            recall_local_command,
            search_local_history,
            local_block_search::search_local_blocks,
            ack_shell,
            recover_shell,
            write_shell,
            resize_shell,
            set_shell_window_pixels,
            key_shell,
            mouse_shell,
            focus_shell,
            submit_local_chat,
            local_workflows::local_workflow_library,
            local_workflows::insert_local_workflow,
            text_shell,
            paste_shell,
            close_shell,
            agent_workspace::workspace_catalog,
            agent_workspace::workspace_remove_cli,
            agent_workspace::workspace_attach,
            agent_workspace::workspace_terminal,
            agent_workspace::workspace_chat,
            agent_workspace::workspace_native,
            agent_workspace::workspace_mission,
            agent_workspace::workspace_tools,
            agent_workspace::workspace_queue,
            agent_workspace::workspace_remote,
            notebook_panel::local_notebook,
            output_review::compare_local_output,
            output_review::export_local_output,
            vim9_workbench::local_vim9,
            ssh_terminal::ssh_terminal,
            ssh_files::ssh_files,
            remote_desktop::remote_desktop
        ])
}

fn main() {
    jterm_core::identity::init(jterm_core::identity::AppIdentity {
        app_name: "aether",
        app_id: "local.jterm.frame-lab",
        app_version: env!("CARGO_PKG_VERSION"),
    });
    if let Some(code) = jterm_core::agent_workspace::run_embedded_if_requested() {
        std::process::exit(code);
    }
    let builder = application(tauri::Builder::default());
    #[cfg(target_os = "linux")]
    let builder = builder.setup(|app| {
        if let Some(window) = app.get_webview_window("main") {
            window.with_webview(|platform| {
                use webkit2gtk::{InputMethodContextExt, WebViewExt};
                if let Some(context) = platform.inner().input_method_context() {
                    // Wry disables preedit for generic popup positioning.
                    // Terminal composition needs native start/update events to
                    // capture its attachment before a later commit. Preserve
                    // that boundary instead of admitting anonymous end events.
                    context.set_enable_preedit(true);
                }
            })?;
        }
        if companion_host::shared_mode_requested() {
            if let Some(window) = app.get_webview_window("main") {
                companion::install(&window)?;
            }
        }
        Ok(())
    });
    builder
        .run(tauri::generate_context!())
        .expect("Tauri runtime");
}

#[cfg(test)]
mod ipc_tests;
