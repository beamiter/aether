//! Window-owned control metadata for the JWM native viewer.
use crate::jwm_remote_session::{
    ClipboardOptions, ClipboardStatus, ConnectionOptions, RemoteDesktop, Scaling, Selection,
    SourceChoice, Viewer, X11EmbedTarget, TICK,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{Manager, Runtime, WindowEvent};

const MAIN: &str = "main";
const SHUTDOWN_LIMIT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    Inspect,
    SetVisible {
        visible: bool,
    },
    Connect {
        generation: u32,
        options: Options,
    },
    SelectSource {
        generation: u32,
        selection: Selection,
    },
    ReleaseInput {
        generation: u32,
    },
    RequestInput {
        generation: u32,
        selection: Selection,
    },
    Disconnect {
        generation: u32,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Options {
    address: String,
    key_file: String,
    viewer: String,
    scaling: String,
    control: bool,
    #[serde(default)]
    reconnect_automatically: bool,
    #[serde(default)]
    wayland_gestures: bool,
    #[serde(default)]
    clipboard: Option<ClipboardOptions>,
}

impl TryFrom<Options> for ConnectionOptions {
    type Error = &'static str;

    fn try_from(options: Options) -> Result<Self, Self::Error> {
        let address = options.address.trim();
        if address.is_empty() || address.len() > 255 || address.chars().any(char::is_control) {
            return Err("Enter a valid host name or address.");
        }
        if options.key_file.len() > 4096 || options.key_file.contains('\0') {
            return Err("Enter a valid private key path.");
        }
        let backend = Viewer::CHOICES
            .into_iter()
            .find(|viewer| viewer.to_string().eq_ignore_ascii_case(&options.viewer))
            .ok_or("Unknown viewer backend.")?;
        let scaling = Scaling::CHOICES
            .into_iter()
            .find(|scale| scale.to_string().eq_ignore_ascii_case(&options.scaling))
            .ok_or("Unknown scaling mode.")?;
        if options.wayland_gestures && (!options.control || backend != Viewer::Wayland) {
            return Err("Gestures require the Wayland viewer and keyboard and pointer control.");
        }
        if options.clipboard.is_some() && backend != Viewer::X11 {
            return Err("Clipboard sharing requires the X11 viewer.");
        }

        let connection = Self {
            address: address.to_owned(),
            key_file: options.key_file,
            backend,
            scaling,
            control: options.control,
            reconnect_automatically: options.reconnect_automatically,
            wayland_gestures: options.wayland_gestures,
            clipboard: options.clipboard,
            x11_embed: None,
        };
        connection.validate_reconnect()?;
        Ok(connection)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    generation: u32,
    active: bool,
    closing: bool,
    embedded: bool,
    release_available: bool,
    request_available: bool,
    status: &'static str,
    feedback: String,
    metrics: String,
    clipboard_requested: bool,
    clipboard: Option<ClipboardStatus>,
    current_source: Option<SourceChoice>,
    sources: Vec<SourceChoice>,
}

#[derive(Default)]
pub(crate) struct WindowRemote {
    generation: u32,
    retired: bool,
    visible: bool,
    embedded: bool,
    session: RemoteDesktop,
}

impl WindowRemote {
    fn apply<R: Runtime>(
        &mut self,
        action: Action,
        window: &tauri::WebviewWindow<R>,
    ) -> Result<Status, String> {
        if self.retired {
            return Err("The remote desktop window is closing.".into());
        }
        self.session.refresh();
        self.remove_inactive_pane();
        match action {
            Action::Inspect => {}
            Action::SetVisible { visible } => {
                self.visible = visible;
                if self.embedded {
                    if !visible {
                        self.session.release_input();
                    }
                    native_pane::set_visible(visible);
                }
            }
            Action::Connect {
                generation,
                options,
            } => {
                self.require_generation(generation)?;
                if self.session.active() {
                    return Err("Disconnect the active viewer before connecting again.".into());
                }
                let mut options = ConnectionOptions::try_from(options)?;
                let next_generation = self
                    .generation
                    .checked_add(1)
                    .ok_or("Remote connection generation exhausted.")?;
                if options.backend == Viewer::X11 {
                    options.x11_embed = native_pane::prepare(window, self.visible)?;
                }
                self.embedded = options.x11_embed.is_some();
                self.generation = next_generation;
                self.session.connect(options);
                self.remove_inactive_pane();
            }
            Action::SelectSource {
                generation,
                selection,
            } => {
                self.require_active(generation)?;
                let source = self
                    .session
                    .sources()
                    .iter()
                    .find(|source| source.selection == selection)
                    .cloned()
                    .ok_or("The source list changed. Select a current source.")?;
                self.session.select_source(&source);
            }
            Action::ReleaseInput { generation } => {
                self.require_active(generation)?;
                self.session.release_input();
            }
            Action::RequestInput {
                generation,
                selection,
            } => {
                self.require_active(generation)?;
                if !self.session.can_request_input()
                    || self
                        .session
                        .current_source()
                        .is_none_or(|source| source.selection != selection)
                {
                    return Err("Input is unavailable for the currently presented source.".into());
                }
                self.session.request_input(&selection);
            }
            Action::Disconnect { generation } => {
                self.require_active(generation)?;
                self.session.disconnect();
            }
        }
        Ok(self.status())
    }

    fn remove_inactive_pane(&mut self) {
        if self.embedded && !self.session.active() {
            native_pane::remove();
            self.embedded = false;
        }
    }

    fn require_generation(&self, generation: u32) -> Result<(), String> {
        if generation == self.generation {
            Ok(())
        } else {
            Err("The remote session changed.".into())
        }
    }

    fn require_active(&self, generation: u32) -> Result<(), String> {
        self.require_generation(generation)?;
        if self.session.active() && !self.session.closing() {
            Ok(())
        } else {
            Err("The remote viewer is closing or disconnected.".into())
        }
    }

    fn status(&self) -> Status {
        Status {
            generation: self.generation,
            active: self.session.active(),
            closing: self.session.closing(),
            embedded: self.embedded,
            release_available: self.session.can_release_input(),
            request_available: self.session.can_request_input(),
            status: self.session.status(),
            feedback: self.session.feedback().to_owned(),
            metrics: self.session.metrics().to_owned(),
            clipboard_requested: self.session.clipboard_requested(),
            clipboard: self.session.clipboard().cloned(),
            current_source: self.session.current_source().cloned(),
            sources: self.session.sources().to_vec(),
        }
    }

    fn retire(&mut self) {
        self.retired = true;
        self.session.disconnect();
    }
}

#[tauri::command]
pub(crate) async fn remote_desktop<R: Runtime>(
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, Arc<Mutex<WindowRemote>>>,
    action: Action,
) -> Result<Status, String> {
    if window.label() != MAIN {
        return Err("Remote desktop belongs to the main application window.".into());
    }
    let state = state.inner().clone();
    let (reply, mut result) = tauri::async_runtime::channel(1);
    let owner = window.clone();
    window
        .run_on_main_thread(move || {
            let outcome = state
                .lock()
                .map_err(|_| "Remote desktop is unavailable.".to_owned())
                .and_then(|mut remote| remote.apply(action, &owner));
            let _ = reply.try_send(outcome);
        })
        .map_err(|error| error.to_string())?;
    result
        .recv()
        .await
        .ok_or_else(|| "Remote desktop window is unavailable.".to_owned())?
}

pub(crate) fn window_event<R: Runtime>(window: &tauri::Window<R>, event: &WindowEvent) {
    if window.label() != MAIN {
        return;
    }
    let Some(state) = window.try_state::<Arc<Mutex<WindowRemote>>>() else {
        return;
    };
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            let Ok(mut remote) = state.lock() else {
                return;
            };
            remote.session.refresh();
            let already_retired = remote.retired;
            remote.retire();
            if remote.session.active() {
                api.prevent_close();
                if !already_retired {
                    let window = window.clone();
                    let state = state.inner().clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        finish_close(&state);
                        let _ = window.destroy();
                    });
                }
            }
        }
        WindowEvent::Destroyed => {
            if let Ok(mut remote) = state.lock() {
                remote.retire();
                native_pane::remove();
            }
        }
        _ => {}
    }
}

#[cfg(target_os = "linux")]
mod native_pane {
    use super::X11EmbedTarget;
    use gtk::prelude::*;
    use std::{cell::RefCell, num::NonZeroU32};

    // GTK widgets stay on the main thread. The controller accepts only the
    // one Aether main window, which owns the sole native pane.
    thread_local! {
        static SOCKET: RefCell<Option<gtk::Socket>> = const { RefCell::new(None) };
    }

    pub(super) fn prepare<R: tauri::Runtime>(
        window: &tauri::WebviewWindow<R>,
        visible: bool,
    ) -> Result<Option<X11EmbedTarget>, String> {
        let native = window.gtk_window().map_err(|error| error.to_string())?;
        let display = WidgetExt::display(&native);
        if !display.is::<gdkx11::X11Display>() {
            return Ok(None);
        }
        let container = window.default_vbox().map_err(|error| error.to_string())?;
        let socket = gtk::Socket::new();
        // JWM acquires input explicitly on a click inside the image.
        socket.set_can_focus(false);
        socket.connect_plug_removed(|socket| {
            socket.hide();
            true
        });
        container.pack_end(&socket, true, true, 0);
        socket.show();
        container.check_resize();
        let parent = u32::try_from(socket.id()).ok().and_then(NonZeroU32::new);
        let Some(parent) = parent else {
            container.remove(&socket);
            return Err("Could not create the native video pane.".into());
        };
        socket.set_visible(visible);
        display.flush();
        SOCKET.with(|slot| slot.replace(Some(socket)));
        Ok(Some(X11EmbedTarget {
            display: display.name().to_string(),
            parent,
        }))
    }

    pub(super) fn set_visible(visible: bool) {
        SOCKET.with_borrow(|slot| {
            if let Some(socket) = slot {
                socket.set_visible(visible);
            }
        });
    }

    pub(super) fn remove() {
        if let Some(socket) = SOCKET.with(|slot| slot.take()) {
            if let Some(parent) = socket.parent().and_downcast::<gtk::Box>() {
                parent.remove(&socket);
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod native_pane {
    use super::X11EmbedTarget;

    pub(super) fn prepare<R: tauri::Runtime>(
        _: &tauri::WebviewWindow<R>,
        _: bool,
    ) -> Result<Option<X11EmbedTarget>, String> {
        Ok(None)
    }

    pub(super) fn set_visible(_: bool) {}
    pub(super) fn remove() {}
}

fn finish_close(state: &Mutex<WindowRemote>) {
    let deadline = Instant::now() + SHUTDOWN_LIMIT;
    while Instant::now() < deadline {
        {
            let Ok(mut remote) = state.lock() else {
                return;
            };
            remote.session.refresh();
            if !remote.session.active() {
                return;
            }
        }
        std::thread::sleep(TICK);
    }
}
