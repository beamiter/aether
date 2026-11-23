use gdk_wayland_sys as gdk_wayland;
use gtk::{gdk, glib, prelude::*};
use std::{ffi::c_void, mem, ptr::NonNull};
use wayland_client::{
    backend::{Backend, ObjectId},
    protocol::{wl_registry, wl_surface::WlSurface},
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
};
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};

pub(super) fn surface(window: &gdk::Window) -> Option<NonNull<c_void>> {
    // The GDK and Wayland calls run on GTK's thread while the window is mapped.
    unsafe {
        let kind = glib::translate::from_glib(gdk_wayland::gdk_wayland_window_get_type());
        if !window.type_().is_a(kind) || window.is_destroyed() {
            return None;
        }
        NonNull::new(gdk_wayland::gdk_wayland_window_get_wl_surface(
            window.as_ptr().cast(),
        ))
    }
}

pub(super) struct Receipts {
    events: Events,
    queue: EventQueue<Events>,
    connection: Connection,
    _registry: wl_registry::WlRegistry,
    display: gdk::Display,
}

#[derive(Default)]
struct Events {
    presentation: Option<wp_presentation::WpPresentation>,
    completed: Vec<(u64, bool)>,
}

impl Receipts {
    pub(super) fn new(display: gdk::Display) -> Option<Self> {
        // GDK retains ownership of the connection and continues reading its socket.
        // The queue and guest backend drop before our retained GDK display.
        let connection = unsafe {
            let kind = glib::translate::from_glib(gdk_wayland::gdk_wayland_display_get_type());
            if !display.type_().is_a(kind) || display.is_closed() {
                return None;
            }
            let raw = gdk_wayland::gdk_wayland_display_get_wl_display(display.as_ptr().cast());
            let raw = NonNull::new(raw)?;
            Connection::from_backend(Backend::from_foreign_display(raw.as_ptr().cast()))
        };
        let queue = connection.new_event_queue();
        let registry = connection.display().get_registry(&queue.handle(), ());
        Some(Self {
            events: Events::default(),
            queue,
            connection,
            _registry: registry,
            display,
        })
    }

    pub(super) fn ready(&self) -> bool {
        self.events.presentation.is_some() && !self.display.is_closed()
    }

    pub(super) fn request(&self, window: &gdk::Window, serial: u64) -> Option<()> {
        let presentation = self.events.presentation.as_ref()?;
        if self.display.is_closed() || window.display() != self.display {
            return None;
        }
        let raw = surface(window)?;
        // The borrowed proxy is used only for this request. GTK may replace it
        // on unmap. The new feedback object is owned by our separate event queue.
        let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), raw.as_ptr().cast()).ok()? };
        let surface = WlSurface::from_id(&self.connection, id).ok()?;
        presentation.feedback(&surface, &self.queue.handle(), serial);
        Some(())
    }

    pub(super) fn poll(&mut self) -> Vec<(u64, bool)> {
        if self.display.is_closed() || self.queue.dispatch_pending(&mut self.events).is_err() {
            self.events.presentation = None;
            self.events.completed.clear();
        } else {
            let _ = self.connection.flush();
        }
        mem::take(&mut self.events.completed)
    }
}

impl Drop for Receipts {
    fn drop(&mut self) {
        if !self.display.is_closed() {
            if let Some(presentation) = self.events.presentation.take() {
                presentation.destroy();
            }
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Events {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            if interface == wp_presentation::WpPresentation::interface().name
                && state.presentation.is_none()
            {
                state.presentation = Some(registry.bind(name, 1, queue, ()));
            }
        }
    }
}

wayland_client::delegate_noop!(Events: ignore wp_presentation::WpPresentation);

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, u64> for Events {
    fn event(
        state: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        serial: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wp_presentation_feedback::Event;
        match event {
            Event::Presented { .. } => state.completed.push((*serial, true)),
            Event::Discarded => state.completed.push((*serial, false)),
            _ => {}
        }
    }
}
