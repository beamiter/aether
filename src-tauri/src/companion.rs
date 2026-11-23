//! The native Eva slot belongs to the main window. The terminal relay and
//! WebView never receive a companion body token or send command contents.
mod presentation;

use crate::LocalSessions;
use companion_host::{
    atlas, ContextRef, Rect, Request, RetirementCapability, SharedHost, TaskStatus, Token,
};
use gtk::{cairo, glib, pango, prelude::*};
use presentation::{Presentation, Surface};
use std::{
    cell::RefCell,
    fmt::Write,
    rc::{Rc, Weak},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{Manager, WebviewWindow, Window, WindowEvent};

const FEEDBACK_DURATION: Duration = Duration::from_secs(2);

thread_local! {
    static MAIN_SLOT: RefCell<Weak<RefCell<Companion>>> = const { RefCell::new(Weak::new()) };
}

pub(crate) struct ShellView {
    pub session: String,
    pub completed: Option<u64>,
    pub exit: Option<i32>,
}

struct Outcome {
    sequence: u64,
    success: bool,
    since: Instant,
    reported: bool,
}

#[derive(Default)]
struct Feedback {
    session: Option<String>,
    last_completed: Option<u64>,
    latest: Option<Outcome>,
}

impl Feedback {
    fn observe(&mut self, view: Option<ShellView>, now: Instant) {
        let Some(view) = view else {
            *self = Self::default();
            return;
        };
        if self.session.as_ref() != Some(&view.session) {
            self.session = Some(view.session);
            self.last_completed = view.completed;
            self.latest = None;
            return;
        }
        if view.completed > self.last_completed {
            self.last_completed = view.completed;
            self.latest = view
                .completed
                .zip(view.exit)
                .map(|(sequence, code)| Outcome {
                    sequence,
                    success: code == 0,
                    since: now,
                    reported: false,
                });
        }
        if self
            .latest
            .as_ref()
            .is_some_and(|outcome| now.duration_since(outcome.since) >= FEEDBACK_DURATION)
        {
            self.latest = None;
        }
    }
}

#[derive(PartialEq, Eq)]
struct Scene {
    token: Token,
    skin: atlas::Skin,
    pose: atlas::Pose,
}

#[derive(PartialEq)]
struct Advertised {
    admission: u64,
    viewport: Rect,
    session: Option<String>,
}

#[derive(Default)]
struct Companion {
    presentation: Presentation,
    client: Option<SharedHost>,
    feedback: Feedback,
    advertised: Option<Advertised>,
    revision: u64,
    scene: Option<Scene>,
    clearance_requested: Option<(Token, u64)>,
    closing: bool,
    close_ready: bool,
}

impl Companion {
    fn request_close(&mut self) -> bool {
        if self.close_ready {
            return false;
        }
        let Some(client) = &mut self.client else {
            return false;
        };
        client.poll();
        if !client.gate.has_connection()
            && client.gate.possible_body_token().is_none()
            && !self.presentation.holds_body()
        {
            return false;
        }
        if !self.closing {
            self.closing = true;
            client.send(Request::Withdraw);
        }
        true
    }

    fn cancel_close(&mut self) {
        if let Some(client) = &mut self.client {
            client.cancel_withdrawal();
        }
        self.closing = false;
        self.advertised = None;
    }

    fn update(&mut self, area: &gtk::DrawingArea, visible: bool, view: Option<ShellView>) {
        let surface = Surface::for_area(area);
        if self
            .presentation
            .select(surface.clone(), self.client.is_some())
        {
            area.queue_draw();
        }
        let eligible = visible && self.presentation.matches(surface.as_ref()) && !self.closing;
        if let Some(client) = &mut self.client {
            client.poll();
        }
        self.presentation
            .presented(self.client.as_mut(), surface.as_ref());
        self.feedback
            .observe(view.filter(|_| eligible), Instant::now());

        if self.client.is_none() && self.feedback.session.is_some() && self.presentation.proven() {
            self.client = Some(SharedHost::from_env(
                "aether",
                RetirementCapability::NativePresented,
                || {},
            ));
        }
        let Some(client) = self.client.as_mut() else {
            if eligible && !self.presentation.proven() && self.presentation.needs_draw() {
                area.queue_draw();
            }
            return;
        };
        if !self.closing && client.gate.has_connection() {
            let viewport = Rect {
                x: 0.0,
                y: 0.0,
                width: area.allocated_width().max(1).into(),
                height: area.allocated_height().max(1).into(),
            };
            let next = Advertised {
                admission: client.gate.admission_generation(),
                viewport,
                session: self.feedback.session.clone(),
            };
            if self.advertised.as_ref() != Some(&next) {
                self.revision += 1;
                let focused = next.session.is_some();
                client.send(Request::Geometry {
                    revision: self.revision,
                    viewport,
                    candidates: if focused { vec![viewport] } else { vec![] },
                    exclusions: vec![],
                });
                let pane = next.session.as_deref().unwrap_or("no-terminal");
                publish_context(client, pane, self.revision, None);
                client.send(Request::Focus { focused });
                self.advertised = Some(next);
            }
        }
        let token = client
            .gate
            .token()
            .filter(|token| self.feedback.session.is_some() && self.presentation.can_draw(token));
        let next_scene = token.cloned().map(|token| Scene {
            token,
            skin: atlas::Skin::from_key(client.gate.skin()).unwrap_or(atlas::Skin::Cat),
            pose: if client.gate.quiet() {
                atlas::Pose::Idle
            } else if let Some(outcome) = &self.feedback.latest {
                if outcome.success {
                    atlas::Pose::CelebrateBriefly
                } else {
                    atlas::Pose::GuardFailure
                }
            } else {
                atlas::Pose::Idle
            },
        });
        if next_scene.is_some() && !client.gate.body_presentation_pending() {
            if let Some(outcome) = self
                .feedback
                .latest
                .as_mut()
                .filter(|outcome| !outcome.reported)
            {
                let pane = self
                    .feedback
                    .session
                    .as_deref()
                    .expect("scene has a local shell");
                let task = jterm_core::companion_host::host_scoped_task_id(
                    client.gate.host_id(),
                    pane,
                    outcome.sequence,
                );
                self.revision += 1;
                publish_context(client, pane, self.revision, Some(task.clone()));
                client.send(Request::TaskObservation {
                    task_id: task,
                    context_revision: self.revision,
                    status: if outcome.success {
                        TaskStatus::Succeeded
                    } else {
                        TaskStatus::Failed
                    },
                });
                outcome.reported = true;
            }
        }
        let clearance = client
            .gate
            .pending_retirement_boundary()
            .map(|(token, after)| (token.clone(), after));
        if self.scene != next_scene || self.clearance_requested != clearance {
            self.scene = next_scene;
            self.clearance_requested = clearance;
            area.queue_draw();
        } else if self.presentation.matches(surface.as_ref())
            && self.presentation.needs_draw()
            && (self.clearance_requested.is_some()
                || (self.scene.is_some() && client.gate.body_presentation_pending()))
        {
            area.queue_draw();
        }
        if self.closing && client.gate.withdrawal_accepted() && !self.presentation.holds_body() {
            self.close_ready = true;
        }
    }

    fn status(&self, enabled: bool) -> &str {
        if self.closing {
            "Waiting for Eva to leave this window before closing."
        } else if !enabled {
            ""
        } else if !self.presentation.supported_backend() {
            "Eva requires Wayland presentation feedback."
        } else if !self.presentation.proven() {
            "Waiting for native presentation feedback."
        } else if self.feedback.session.is_none() {
            "Eva follows the focused local shell."
        } else if let Some(client) = &self.client {
            if self.scene.is_some() && !client.gate.body_presentation_pending() {
                match self.feedback.latest.as_ref() {
                    Some(result) if result.success => "Command succeeded",
                    Some(_) => "Command failed",
                    None => "Eva ready",
                }
            } else {
                client.gate.status()
            }
        } else {
            "Connecting to Eva."
        }
    }

    fn draw(&mut self, area: &gtk::DrawingArea, cr: &cairo::Context) {
        if !self.presentation.matches(Surface::for_area(area).as_ref()) {
            return;
        }
        let width = f64::from(area.allocated_width());
        let height = f64::from(area.allocated_height());
        let whole_surface = cr.copy_clip_rectangle_list().is_ok_and(|rects| {
            rects.iter().any(|rect| {
                rect.x() <= 0.0
                    && rect.y() <= 0.0
                    && rect.x() + rect.width() >= width
                    && rect.y() + rect.height() >= height
            })
        });
        cr.set_source_rgb(0.043, 0.067, 0.094);
        if cr.paint().is_err() {
            self.presentation.discard_current_frame();
            return;
        }
        let allowed = self.client.as_ref().and_then(|client| client.gate.token());
        let body = self
            .scene
            .as_ref()
            .filter(|scene| !self.closing && allowed == Some(&scene.token))
            .and_then(|scene| {
                // Record possible pixels before drawing, even if Cairo fails or
                // this damage is smaller than the dedicated native slot.
                self.presentation.mark_body_drawn(&scene.token);
                draw_atlas(area, cr, scene, width, height).then(|| scene.token.clone())
            });
        if whole_surface && cr.status().is_ok() {
            self.presentation.painted(self.client.as_mut(), body);
        } else {
            self.presentation.discard_current_frame();
        }
    }
}

fn publish_context(client: &mut SharedHost, pane: &str, revision: u64, task_id: Option<String>) {
    client.send(Request::ContextObserved {
        context: ContextRef {
            host_id: client.gate.host_id().into(),
            pane_id: pane.into(),
            task_id,
            project_id: None,
            revision,
        },
    });
}

fn draw_atlas(
    area: &gtk::DrawingArea,
    cr: &cairo::Context,
    scene: &Scene,
    width: f64,
    height: f64,
) -> bool {
    let mut request = atlas::Request::new(scene.skin, scene.pose);
    request.motion = atlas::Motion::Static;
    let Some(frame) = atlas::sample(request) else {
        return false;
    };
    let layout = area.create_pango_layout(None);
    layout.set_font_description(Some(&pango::FontDescription::from_string("Monospace 9")));
    let mut markup = String::new();
    let palette = scene.skin.palette(false);
    for row in frame.cells.chunks(frame.width) {
        for cell in row {
            if cell.glyph == ' ' {
                markup.push(' ');
                continue;
            }
            let [red, green, blue] = palette.color(cell.ink);
            let glyph = glib::markup_escape_text(&cell.glyph.to_string());
            let _ = write!(
                markup,
                "<span foreground=\"#{red:02x}{green:02x}{blue:02x}\">{glyph}</span>"
            );
        }
        markup.push('\n');
    }
    layout.set_markup(markup.trim_end());
    let (layout_width, layout_height) = layout.pixel_size();
    let scale = ((width - 8.0) / f64::from(layout_width.max(1)))
        .min((height - 8.0) / f64::from(layout_height.max(1)))
        .min(1.0);
    if scale <= 0.0 || cr.save().is_err() {
        return false;
    }
    cr.translate((width - f64::from(layout_width) * scale) / 2.0, 4.0);
    cr.scale(scale, scale);
    gtk::render_layout(&area.style_context(), cr, 0.0, 0.0, &layout);
    cr.restore().is_ok()
}

pub(crate) fn install(window: &WebviewWindow) -> tauri::Result<()> {
    let native = window.gtk_window()?;
    let container = window.default_vbox()?;
    let sessions = window.state::<Arc<Mutex<LocalSessions>>>().inner().clone();
    let state = Rc::new(RefCell::new(Companion {
        presentation: Presentation::new(native.display()),
        ..Companion::default()
    }));
    MAIN_SLOT.with_borrow_mut(|slot| *slot = Rc::downgrade(&state));

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let toggle = gtk::ToggleButton::with_label("Eva");
    toggle.set_active(true);
    let area = gtk::DrawingArea::new();
    area.set_has_window(false);
    area.set_size_request(152, 70);
    let status = gtk::Label::new(None);
    status.set_line_wrap(true);
    let cancel = gtk::Button::with_label("Cancel close");
    row.pack_start(&toggle, false, false, 4);
    row.pack_start(&area, false, false, 0);
    row.pack_start(&status, true, true, 0);
    row.pack_end(&cancel, false, false, 4);
    container.pack_end(&row, false, false, 0);
    row.show_all();
    cancel.hide();

    let kept_state = state.clone();
    area.connect_draw(move |area, cr| {
        kept_state.borrow_mut().draw(area, cr);
        glib::Propagation::Proceed
    });
    let cancel_state = Rc::downgrade(&state);
    cancel.connect_clicked(move |_| {
        if let Some(state) = cancel_state.upgrade() {
            state.borrow_mut().cancel_close();
        }
    });

    let weak = Rc::downgrade(&state);
    let (native, area) = (native.downgrade(), area.downgrade());
    let window = window.clone();
    glib::timeout_add_local(Duration::from_millis(32), move || {
        let (Some(native), Some(area), Some(state)) =
            (native.upgrade(), area.upgrade(), weak.upgrade())
        else {
            return glib::ControlFlow::Break;
        };
        let Ok(sessions) = sessions.try_lock() else {
            return glib::ControlFlow::Continue;
        };
        let view = sessions.companion_shell();
        drop(sessions);
        let mut state = state.borrow_mut();
        let enabled = toggle.is_active();
        state.update(&area, enabled && native.is_active(), view);
        let message = state.status(enabled);
        if status.text().as_str() != message {
            status.set_text(message);
        }
        cancel.set_visible(state.closing);
        if state.close_ready {
            drop(state);
            let _ = window.close();
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
    Ok(())
}

pub(crate) fn window_event<R: tauri::Runtime>(window: &Window<R>, event: &WindowEvent) -> bool {
    if window.label() != "main" {
        return false;
    }
    let WindowEvent::CloseRequested { api, .. } = event else {
        return false;
    };
    if MAIN_SLOT
        .with_borrow(|slot| slot.upgrade())
        .is_some_and(|state| state.borrow_mut().request_close())
    {
        api.prevent_close();
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(session: &str, completed: u64, exit: Option<i32>) -> Option<ShellView> {
        Some(ShellView {
            session: session.into(),
            completed: Some(completed),
            exit,
        })
    }

    #[test]
    fn shell_switch_and_detach_baseline_old_results() {
        let now = Instant::now();
        let mut feedback = Feedback::default();
        feedback.observe(view("left", 1, Some(1)), now);
        assert!(feedback.latest.is_none());
        feedback.observe(view("left", 2, Some(0)), now);
        assert!(feedback.latest.as_ref().unwrap().success);

        feedback.observe(view("right", 2, Some(0)), now);
        assert!(feedback.latest.is_none());
        feedback.observe(view("right", 3, Some(2)), now);
        assert!(!feedback.latest.as_ref().unwrap().success);
        feedback.observe(None, now);
        feedback.observe(view("right", 3, Some(2)), now);
        assert!(feedback.latest.is_none());
    }

    #[test]
    fn only_reported_exit_codes_produce_live_feedback() {
        let now = Instant::now();
        let mut feedback = Feedback::default();
        feedback.observe(view("left", 1, Some(0)), now);
        feedback.observe(view("left", 2, None), now);
        assert!(feedback.latest.is_none());
        feedback.observe(view("left", 3, Some(0)), now);
        assert!(feedback.latest.is_some());
        feedback.observe(view("left", 3, Some(0)), now + FEEDBACK_DURATION);
        assert!(feedback.latest.is_none());
    }
}
