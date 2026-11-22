//! Native chooser, authenticated Vim9 bridge, and one dedicated compatibility PTY.

use jterm_core::vim9::{
    Vim9Command, Vim9EntryRequest, Vim9RenderFrame, Vim9RenderMetrics, Vim9RenderUpdate,
    Vim9RenderWatch, Vim9Session,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::Duration;
use tauri::WebviewWindow;
use tauri_terminal_frame_bridge::{live::LiveBridge, protocol_v4::Counter, semantic::Attachment};
use uuid::Uuid;

mod project;
mod snapshot;

const ROWS: usize = 24;
const COLUMNS: usize = 80;
const DRAIN_INTERVAL: Duration = Duration::from_millis(25);
const METRICS: Vim9RenderMetrics = Vim9RenderMetrics {
    origin_x: 0.0,
    origin_y: 0.0,
    cell_width: 12.0,
    line_height: 24.0,
};

#[derive(Default)]
pub struct Workbench {
    choosing: bool,
    editor: Option<Arc<Mutex<Editor>>>,
}

struct Editor {
    vim: Vim9Session,
    selected: String,
    initial_position: Option<(u64, Option<u64>)>,
    render: Vim9RenderWatch,
    latest: Option<Vim9RenderUpdate>,
    render_enabled: bool,
    terminal: LiveBridge,
    attachment: Attachment,
    transport_error: Option<String>,
    project: project::Tools,
}

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Request {
    Inspect,
    Open,
    OpenSnapshot {
        reference: tauri_terminal_frame_bridge::file_reference::FileReference,
        line: Option<u32>,
        column: Option<u32>,
    },
    Poll {
        entry_id: Uuid,
        terminal_revision: Option<Counter>,
        render_generation: Option<String>,
        tools_revision: Option<u32>,
    },
    Lookup {
        entry_id: Uuid,
        query: String,
        folder: String,
        refresh: bool,
    },
    Choose {
        entry_id: Uuid,
        menu: Uuid,
        key: String,
    },
    Job {
        entry_id: Uuid,
        job: project::Job,
    },
    Key {
        entry_id: Uuid,
        name: String,
        shift: bool,
        alt: bool,
        ctrl: bool,
    },
    Text {
        entry_id: Uuid,
        text: String,
    },
    Paste {
        entry_id: Uuid,
        text: String,
    },
    Save {
        entry_id: Uuid,
    },
    Quit {
        entry_id: Uuid,
    },
    Release {
        entry_id: Uuid,
    },
}

#[derive(Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum Response {
    Idle,
    State(Box<EditorView>),
    Accepted,
    Choices(project::Choices),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorView {
    entry_id: Uuid,
    owner: Attachment,
    connected: bool,
    pty_live: bool,
    exited: bool,
    drained: bool,
    modified: bool,
    file: String,
    mode: String,
    line: u64,
    byte_column: u64,
    window_count: usize,
    activity: Option<project::ActivityUpdate>,
    error: Option<String>,
    terminal: Option<String>,
    render: Option<RenderView>,
}

#[derive(Serialize)]
pub struct RenderView {
    generation: String,
    frame: Option<Arc<Vim9RenderFrame>>,
    metrics: Vim9RenderMetrics,
}

#[tauri::command]
pub async fn local_vim9<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    state: tauri::State<'_, Arc<Mutex<Workbench>>>,
    request: Request,
) -> Result<Response, String> {
    super::require_terminal_window(&window)?;
    let state = state.inner().clone();
    if matches!(request, Request::Open | Request::OpenSnapshot { .. }) {
        let snapshot = match request {
            Request::OpenSnapshot {
                reference,
                line,
                column,
            } => Some(snapshot::Snapshot::new(reference, line, column)?),
            _ => None,
        };
        let _reservation = OpenReservation::acquire(state.clone())?;
        let Some(path) = choose_native_file(&window).await? else {
            return Ok(Response::Idle);
        };
        return super::run_bridge(state, move |host| host.open(path, snapshot)).await;
    }
    super::run_bridge(state, move |host| host.apply(request)).await
}

/// Prevent overlapping native dialogs across asynchronous WebView requests.
struct OpenReservation(Arc<Mutex<Workbench>>);

impl OpenReservation {
    fn acquire(state: Arc<Mutex<Workbench>>) -> Result<Self, String> {
        {
            let mut host = state.lock().map_err(|_| "Workbench unavailable.")?;
            if host.choosing || host.editor.is_some() {
                return Err(
                    "Finish the current Vim9 workbench before opening another file.".into(),
                );
            }
            host.choosing = true;
        }
        Ok(Self(state))
    }
}

impl Drop for OpenReservation {
    fn drop(&mut self) {
        if let Ok(mut host) = self.0.lock() {
            host.choosing = false;
        }
    }
}

#[cfg(target_os = "linux")]
async fn choose_native_file<R: tauri::Runtime>(
    window: &WebviewWindow<R>,
) -> Result<Option<PathBuf>, String> {
    use gtk::prelude::*;
    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let owner = window.clone();
    window
        .run_on_main_thread(move || {
            let parent = match owner.gtk_window() {
                Ok(parent) => parent,
                Err(error) => {
                    let _ = sender.try_send(Err(error.to_string()));
                    return;
                }
            };
            let chooser = gtk::FileChooserNative::new(
                Some("Open local file in Vim9"),
                Some(&parent),
                gtk::FileChooserAction::Open,
                Some("Open"),
                Some("Cancel"),
            );
            chooser.set_local_only(true);
            chooser.set_modal(true);
            // The native chooser lives until its response, including cancellation.
            let lifetime = std::rc::Rc::new(std::cell::RefCell::new(Some(chooser.clone())));
            chooser.connect_response(move |chooser, response| {
                let path = if response == gtk::ResponseType::Accept {
                    chooser
                        .file()
                        .and_then(|file| file.path())
                        .map(Some)
                        .ok_or_else(|| "Select an existing local file.".to_owned())
                } else {
                    Ok(None)
                };
                let _ = sender.try_send(path);
                lifetime.borrow_mut().take();
                chooser.hide();
            });
            chooser.show();
        })
        .map_err(|error| error.to_string())?;
    receiver
        .recv()
        .await
        .ok_or_else(|| "Local file chooser closed.".to_owned())?
}

#[cfg(not(target_os = "linux"))]
async fn choose_native_file<R: tauri::Runtime>(
    _: &WebviewWindow<R>,
) -> Result<Option<PathBuf>, String> {
    Err("Native Vim9 file selection is available on Linux.".into())
}

impl Workbench {
    fn open(
        &mut self,
        path: PathBuf,
        snapshot: Option<snapshot::Snapshot>,
    ) -> Result<Response, String> {
        let path = path
            .canonicalize()
            .map_err(|error| format!("Selected local file is unavailable: {error}"))?;
        if let Some(snapshot) = &snapshot {
            snapshot.verify(&path)?;
        }
        let mut editor = Editor::start(path, snapshot.and_then(|s| s.position))?;
        let view = editor.status(None, None, None)?;
        let editor = Arc::new(Mutex::new(editor));
        start_drain_worker(Arc::downgrade(&editor))?;
        self.editor = Some(editor);
        Ok(Response::State(Box::new(view)))
    }

    fn apply(&mut self, request: Request) -> Result<Response, String> {
        if matches!(request, Request::Inspect) {
            return match &self.editor {
                Some(editor) => Ok(Response::State(Box::new(
                    editor
                        .lock()
                        .map_err(|_| "Vim9 unavailable.")?
                        .status(None, None, None)?,
                ))),
                None => Ok(Response::Idle),
            };
        }
        let editor = self
            .editor
            .as_ref()
            .ok_or("Open a local file in Vim9 first.")?;
        let mut editor = editor.lock().map_err(|_| "Vim9 unavailable.")?;
        match request {
            Request::Poll {
                entry_id,
                terminal_revision,
                render_generation,
                tools_revision,
            } => {
                editor.require_entry(entry_id)?;
                return Ok(Response::State(Box::new(editor.status(
                    terminal_revision,
                    render_generation.as_deref(),
                    tools_revision,
                )?)));
            }
            Request::Lookup {
                entry_id,
                query,
                folder,
                refresh,
            } => {
                editor.require_input(entry_id)?;
                let Editor { vim, project, .. } = &mut *editor;
                return Ok(Response::Choices(
                    project.lookup(vim, &query, &folder, refresh)?,
                ));
            }
            Request::Choose {
                entry_id,
                menu,
                key,
            } => {
                editor.require_input(entry_id)?;
                let Editor { vim, project, .. } = &mut *editor;
                let command = project.choose(vim, menu, &key)?;
                editor.command(command)?;
            }
            Request::Job { entry_id, job } => {
                editor.require_input(entry_id)?;
                let command = job.command(&editor.vim.snapshot())?;
                editor.command(command)?;
            }
            Request::Key {
                entry_id,
                name,
                shift,
                alt,
                ctrl,
            } => {
                let epoch = editor.require_input(entry_id)?;
                editor.terminal.key(epoch, &name, shift, alt, ctrl)?;
            }
            Request::Text { entry_id, text } => {
                let epoch = editor.require_input(entry_id)?;
                editor.terminal.text(epoch, &text)?;
            }
            Request::Paste { entry_id, text } => {
                let epoch = editor.require_input(entry_id)?;
                editor.terminal.paste(epoch, &text)?;
            }
            Request::Save { entry_id } => {
                editor.require_input(entry_id)?;
                editor.command(Vim9Command::Save)?;
            }
            Request::Quit { entry_id } => {
                editor.require_input(entry_id)?;
                editor.command(Vim9Command::Quit)?;
            }
            Request::Release { entry_id } => {
                editor.require_entry(entry_id)?;
                let epoch = editor.attachment.epoch;
                if !editor.terminal.status(epoch)?.drained {
                    return Err(
                        "Quit Vim and let its terminal drain before closing the workbench.".into(),
                    );
                }
                editor.terminal.close(epoch)?;
                editor.vim.revoke();
                drop(editor);
                self.editor.take();
                return Ok(Response::Idle);
            }
            Request::Open | Request::OpenSnapshot { .. } | Request::Inspect => {
                unreachable!("handled before dispatch")
            }
        }
        Ok(Response::Accepted)
    }
}

impl Editor {
    fn start(path: PathBuf, initial_position: Option<(u64, Option<u64>)>) -> Result<Self, String> {
        if !path.is_file() {
            return Err("Select an existing regular local file.".into());
        }
        let selected = path
            .to_str()
            .ok_or("The selected file path is not UTF-8.")?
            .to_owned();
        let vim = Vim9Session::start(Vim9EntryRequest::user_action(path))
            .map_err(|error| error.to_string())?;
        let render = vim.watch_render().map_err(|error| error.to_string())?;
        let launch = vim.launch();
        let argv = std::iter::once(launch.program.as_str())
            .chain(launch.args.iter().map(String::as_str))
            .map(OsString::from)
            .collect::<Vec<_>>();
        let mut env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        env.remove(std::ffi::OsStr::new("COLUMNS"));
        env.remove(std::ffi::OsStr::new("LINES"));
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        for assignment in &launch.env {
            let (key, value) = assignment
                .split_once('=')
                .ok_or("Invalid Vim9 bootstrap variable.")?;
            env.insert(key.into(), value.into());
        }
        let mut terminal = LiveBridge::local_desktop();
        let epoch = terminal.open_prepared_editor(
            ROWS,
            COLUMNS,
            &argv,
            &launch.cwd,
            &env.into_iter().collect::<Vec<_>>(),
        )?;
        let attachment = terminal.attachment(epoch)?;
        Ok(Self {
            vim,
            selected,
            initial_position,
            render,
            latest: None,
            render_enabled: false,
            terminal,
            attachment,
            transport_error: None,
            project: project::Tools::default(),
        })
    }

    fn require_entry(&self, entry_id: Uuid) -> Result<(), String> {
        // The UUID still owns the compatibility PTY after the GUI bridge
        // revokes its independent command and paint authority.
        if self.vim.entry_id() == entry_id {
            Ok(())
        } else {
            Err("The Vim9 entry has changed.".into())
        }
    }

    fn require_input(&self, entry_id: Uuid) -> Result<u32, String> {
        self.require_entry(entry_id)?;
        let epoch = self.terminal.validate_logical_input(&self.attachment)?;
        if self.transport_error.is_some() || self.terminal.status(epoch)?.exited {
            return Err("The Vim PTY has exited or its input is unavailable.".into());
        }
        Ok(epoch)
    }

    fn command(&mut self, command: Vim9Command) -> Result<(), String> {
        if !self.vim.snapshot().connected {
            return Err("Use the live compatibility terminal to control Vim while its native bridge is offline.".into());
        }
        // Vim's bridge checks its own typeahead before saving. First deliver
        // preceding edits that are still in the host-side PTY FIFO.
        self.terminal.flush_editor_input(&self.attachment)?;
        self.vim.clear_error();
        self.vim
            .send(command)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn status(
        &mut self,
        terminal_revision: Option<Counter>,
        render_generation: Option<&str>,
        tools_revision: Option<u32>,
    ) -> Result<EditorView, String> {
        if let Ok(update) = self.render.recv_timeout(Duration::ZERO) {
            if update.generation > 0 {
                self.latest = Some(update);
            }
        }
        let epoch = self.attachment.epoch;
        let mut editor = self.vim.snapshot();
        if editor.connected && !self.render_enabled {
            self.render_enabled = self
                .vim
                .send(Vim9Command::SetRender { enabled: true })
                .is_ok();
        } else if !editor.connected {
            self.render_enabled = false;
        }
        if editor.connected && !editor.file.is_empty() {
            if let Some((line, column)) = self.initial_position.take() {
                if editor.file == self.selected {
                    self.vim
                        .send(Vim9Command::Goto { line, column })
                        .map_err(|e| e.to_string())?;
                } else {
                    editor.error = Some("Vim changed files before the requested jump.".into());
                }
            }
        }
        let session = self.terminal.status(epoch)?;
        let render = self.latest.as_ref().and_then(|update| {
            let generation = update.generation.to_string();
            (Some(generation.as_str()) != render_generation).then(|| RenderView {
                generation,
                frame: update.frame.clone(),
                metrics: METRICS,
            })
        });
        let terminal = self
            .terminal
            .observe_channel_frame(epoch, terminal_revision)?
            .map(|(_, frame)| frame);
        let activity = tools_revision
            .map(|seen| self.project.activity(self.vim.root(), &mut editor, seen))
            .transpose()?
            .flatten();
        Ok(EditorView {
            entry_id: self.vim.entry_id(),
            owner: self.attachment.clone(),
            connected: editor.connected,
            pty_live: !session.exited
                && self.transport_error.is_none()
                && session.write_error.is_none(),
            exited: session.exited,
            drained: session.drained,
            modified: editor.modified,
            file: if editor.file.is_empty() {
                self.selected.clone()
            } else {
                editor.file
            },
            mode: editor.mode,
            line: editor.line,
            byte_column: editor.column,
            window_count: editor.window_count,
            activity,
            error: self
                .transport_error
                .clone()
                .or(session.write_error)
                .or(editor.error),
            terminal,
            render,
        })
    }
}

fn start_drain_worker(editor: Weak<Mutex<Editor>>) -> Result<(), String> {
    thread::Builder::new()
        .name("aether-vim9-pty".into())
        .spawn(move || loop {
            let active = {
                let Some(editor) = editor.upgrade() else {
                    break;
                };
                let Ok(mut editor) = editor.lock() else {
                    break;
                };
                match editor.terminal.pump_unpublished() {
                    Ok(active) => active,
                    Err(error) => {
                        editor.transport_error = Some(error);
                        false
                    }
                }
            };
            if !active {
                break;
            }
            thread::sleep(DRAIN_INTERVAL);
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}
