//! Agent Workspace shell catalog and views backed by the shared native host.
use jterm_core::agent_workspace::{
    ConversationId, WorkspaceClient, WorkspaceCommand, WorkspaceEvent, WorkspaceReply,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

mod catalog;
mod chat;
mod missions;
mod native_tasks;
mod queue;
mod remote;
mod terminal;
mod tools;

#[derive(Default)]
pub(crate) struct Workspace {
    catalog: Arc<Mutex<catalog::Catalog>>,
    terminal: Arc<Mutex<terminal::Views>>,
    chat: Arc<Mutex<chat::Chats>>,
    native: Arc<Mutex<native_tasks::Tasks>>,
    missions: Arc<Mutex<missions::Missions>>,
    tools: Arc<Mutex<tools::Tools>>,
    queue: Arc<Mutex<queue::Queue>>,
    remote: Arc<Mutex<remote::Viewer>>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cursor {
    generation: u32,
    before: String,
}

#[derive(serde::Serialize)]
pub(crate) struct Receipt<T> {
    request: u32,
    value: T,
}

fn admit(request: u32) -> Result<(), String> {
    if request == 0 {
        return Err("Invalid Workspace request identity".into());
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn workspace_remote<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    request: u32,
    action: remote::Action,
    workspace: tauri::State<'_, Workspace>,
) -> Result<Receipt<remote::Response>, String> {
    super::require_terminal_window(&window)?;
    admit(request)?;
    let value = super::run_bridge(workspace.remote.clone(), move |viewer| {
        viewer.execute(action)
    })
    .await?;
    Ok(Receipt { request, value })
}

fn request_host(
    client: &mut Option<WorkspaceClient>,
    command: WorkspaceCommand,
) -> Result<WorkspaceReply, String> {
    let client = client.get_or_insert_with(WorkspaceClient::new);
    let request = client.send(command)?;
    let deadline = Instant::now() + Duration::from_secs(18);
    while Instant::now() < deadline {
        for WorkspaceEvent::Reply { request_id, result } in client.poll() {
            if request_id == request {
                return result;
            }
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    Err("Workspace host did not confirm the request. Refresh to observe its state.".into())
}

#[tauri::command]
pub(crate) async fn workspace_native<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    request: u32,
    action: native_tasks::Action,
    workspace: tauri::State<'_, Workspace>,
) -> Result<Receipt<native_tasks::Reply>, String> {
    super::require_terminal_window(&window)?;
    admit(request)?;
    let value =
        super::run_bridge(workspace.native.clone(), move |tasks| tasks.execute(action)).await?;
    Ok(Receipt { request, value })
}

#[tauri::command]
pub(crate) async fn workspace_mission<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    request: u32,
    choice: missions::Choice,
    workspace: tauri::State<'_, Workspace>,
) -> Result<Receipt<missions::Reply>, String> {
    super::require_terminal_window(&window)?;
    admit(request)?;
    let source = if let Some(identity) = choice.source().cloned() {
        Some(
            super::run_bridge(workspace.native.clone(), move |tasks| {
                tasks.inspect_selected(&identity)
            })
            .await?,
        )
    } else {
        None
    };
    let value = super::run_bridge(workspace.missions.clone(), move |missions| {
        missions.execute(choice, source)
    })
    .await?;
    Ok(Receipt { request, value })
}

#[tauri::command]
pub(crate) async fn workspace_tools<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    request: u32,
    choice: tools::Choice,
    workspace: tauri::State<'_, Workspace>,
) -> Result<Receipt<tools::Reply>, String> {
    super::require_terminal_window(&window)?;
    admit(request)?;
    let conversation = if let Some(identity) = choice.scope().cloned() {
        Some(
            super::run_bridge(workspace.native.clone(), move |tasks| {
                tasks
                    .inspect_selected(&identity)
                    .map(|snapshot| snapshot.id)
            })
            .await?,
        )
    } else {
        None
    };
    let value = super::run_bridge(workspace.tools.clone(), move |tools| {
        tools.execute(choice, conversation)
    })
    .await?;
    Ok(Receipt { request, value })
}

#[tauri::command]
pub(crate) async fn workspace_queue<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    request: u32,
    choice: queue::Choice,
    workspace: tauri::State<'_, Workspace>,
) -> Result<Receipt<queue::View>, String> {
    super::require_terminal_window(&window)?;
    admit(request)?;
    let source = if let Some(identity) = choice.source().cloned() {
        Some(
            super::run_bridge(workspace.native.clone(), move |tasks| {
                tasks
                    .inspect_selected(&identity)
                    .map(|snapshot| snapshot.id)
            })
            .await?,
        )
    } else {
        None
    };
    let value = super::run_bridge(workspace.queue.clone(), move |queue| {
        queue.execute(choice, source)
    })
    .await?;
    Ok(Receipt { request, value })
}

#[tauri::command]
pub(crate) async fn workspace_chat<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    request: u32,
    action: chat::Action,
    workspace: tauri::State<'_, Workspace>,
) -> Result<Receipt<chat::Reply>, String> {
    super::require_terminal_window(&window)?;
    admit(request)?;
    let value = super::run_bridge(workspace.chat.clone(), move |chat| chat.execute(action)).await?;
    Ok(Receipt { request, value })
}

#[tauri::command]
pub(crate) async fn workspace_catalog<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    request: u32,
    cursor: Option<Cursor>,
    workspace: tauri::State<'_, Workspace>,
) -> Result<Receipt<catalog::Page>, String> {
    super::require_terminal_window(&window)?;
    admit(request)?;
    let value = super::run_bridge(workspace.catalog.clone(), move |catalog| {
        catalog.page(cursor.map(|cursor| (cursor.generation, cursor.before)))
    })
    .await?;
    Ok(Receipt { request, value })
}

#[tauri::command]
pub(crate) async fn workspace_remove_cli<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    request: u32,
    conversation: ConversationId,
    catalog_generation: u32,
    workspace: tauri::State<'_, Workspace>,
) -> Result<Receipt<ConversationId>, String> {
    super::require_terminal_window(&window)?;
    admit(request)?;
    let value = super::run_bridge(workspace.catalog.clone(), move |catalog| {
        catalog.remove(catalog_generation, conversation)
    })
    .await?;
    Ok(Receipt { request, value })
}

#[tauri::command]
pub(crate) async fn workspace_attach<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    request: u32,
    conversation: ConversationId,
    catalog_generation: u32,
    workspace: tauri::State<'_, Workspace>,
) -> Result<Receipt<terminal::Attached>, String> {
    super::require_terminal_window(&window)?;
    admit(request)?;
    let selection = conversation.clone();
    let source = super::run_bridge(workspace.catalog.clone(), move |catalog| {
        catalog.source(catalog_generation, &selection)
    })
    .await?;
    let value = super::run_bridge(workspace.terminal.clone(), move |views| {
        views.attach(request, conversation, &source)
    })
    .await?;
    Ok(Receipt { request, value })
}

#[tauri::command]
pub(crate) async fn workspace_terminal<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    request: u32,
    identity: terminal::Identity,
    operation: terminal::Operation,
    workspace: tauri::State<'_, Workspace>,
) -> Result<Receipt<terminal::Reply>, String> {
    super::require_terminal_window(&window)?;
    admit(request)?;
    let value = super::run_bridge(workspace.terminal.clone(), move |views| {
        views.execute(request, &identity, operation)
    })
    .await?;
    Ok(Receipt { request, value })
}
