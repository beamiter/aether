use jterm_core::{
    agent_sessions::AgentSession,
    agent_workspace::{ConversationId, WorkspaceTerminalAttachment, WorkspaceTerminalEvent},
};
use jterm_engine::{
    frame::CellFrame,
    terminal::{Terminal, MAX_FEED_BYTES},
};
use std::num::NonZeroU64;
use tauri_terminal_frame_bridge::{
    semantic::{self, Attachment, Update},
    WireFrame,
};

const ROWS: u16 = 24;
const COLS: u16 = 80;
const EVENT_BUDGET: usize = 16;

#[derive(Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    conversation: ConversationId,
    owner: Attachment,
}

#[derive(Clone, serde::Serialize)]
pub(crate) struct Attached {
    identity: Identity,
    frame: WireFrame,
}

#[derive(serde::Serialize)]
pub(crate) struct Polled {
    frame: Option<Update>,
    writable: bool,
    status: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, tag = "operation", rename_all = "snake_case")]
pub(crate) enum Operation {
    Poll,
    Ack {
        revision: u32,
    },
    Key {
        name: String,
        shift: bool,
        alt: bool,
        ctrl: bool,
    },
    Text {
        text: String,
    },
    Paste {
        text: String,
    },
    Detach,
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Reply {
    Receipt,
    Polled { update: Polled },
}

#[derive(Default)]
pub(super) struct Views {
    generation: u32,
    active: Option<View>,
    last_detached: Option<Identity>,
}

struct View {
    opening_request: u32,
    receipt: Attached,
    connection: Option<WorkspaceTerminalAttachment>,
    terminal: Terminal,
    accepted: Option<CellFrame>,
    flight: Option<CellFrame>,
    status: String,
    last_input_request: u32,
}

impl Views {
    pub(super) fn attach(
        &mut self,
        request: u32,
        conversation: ConversationId,
        source: &AgentSession,
    ) -> Result<Attached, String> {
        if let Some(view) = &self.active {
            if view.opening_request == request
                && view.receipt.identity.conversation == conversation
                && view.accepted.is_none()
            {
                return Ok(view.receipt.clone());
            }
            return Err("Detach the current Workspace terminal first".into());
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or("Workspace view exhausted")?;
        let epoch = NonZeroU64::new(generation.into()).ok_or("Invalid Workspace view")?;
        let mut terminal = Terminal::with_epoch(ROWS.into(), COLS.into(), epoch)?;
        let (connection, snapshot, screen) =
            WorkspaceTerminalAttachment::attach(source, ROWS, COLS)?;
        if snapshot.id != conversation {
            return Err("Workspace host returned a different conversation".into());
        }
        for bytes in screen.chunks(MAX_FEED_BYTES) {
            // The initial host snapshot restores display state. Terminal
            // query replies are only sent for subsequent live output.
            terminal.feed(bytes)?;
        }
        let owner = Attachment {
            session: snapshot.epoch.replace('-', ""),
            epoch: generation,
            generation,
        };
        let frame = terminal.frame()?;
        let receipt = Attached {
            frame: tauri_terminal_frame_bridge::from_engine(&frame, &owner.session, owner.epoch)?,
            identity: Identity {
                conversation,
                owner,
            },
        };
        self.generation = generation;
        self.active = Some(View {
            opening_request: request,
            receipt: receipt.clone(),
            connection: Some(connection),
            terminal,
            accepted: None,
            flight: Some(frame),
            status: "Attached to the Workspace provider PTY.".into(),
            last_input_request: 0,
        });
        Ok(receipt)
    }

    pub(super) fn execute(
        &mut self,
        request: u32,
        identity: &Identity,
        operation: Operation,
    ) -> Result<Reply, String> {
        if matches!(operation, Operation::Detach) && self.last_detached.as_ref() == Some(identity) {
            return Ok(Reply::Receipt);
        }
        let view = self
            .active
            .as_mut()
            .filter(|view| &view.receipt.identity == identity)
            .ok_or("Stale Workspace terminal attachment")?;
        match operation {
            Operation::Detach => {
                self.last_detached = Some(identity.clone());
                self.active = None;
            }
            Operation::Poll => return view.poll().map(|update| Reply::Polled { update }),
            Operation::Ack { revision } => view.ack(revision)?,
            Operation::Text { text } => {
                view.drain()?;
                let bytes = view.terminal.encode_text(&text)?;
                view.input_for(request)?.input(&bytes)?;
            }
            Operation::Paste { text } => {
                view.drain()?;
                let bytes = view.terminal.encode_paste(&text)?;
                view.input_for(request)?.input_paste(bytes)?;
            }
            Operation::Key {
                name,
                shift,
                alt,
                ctrl,
            } => {
                view.drain()?;
                let bytes = view.terminal.encode_key(&name, shift, alt, ctrl)?;
                view.input_for(request)?.input(&bytes)?;
            }
        }
        Ok(Reply::Receipt)
    }
}

impl View {
    fn ack(&mut self, revision: u32) -> Result<(), String> {
        let matches = |frame: &CellFrame| frame.stamp().revision == u64::from(revision);
        if self.flight.is_none() && self.accepted.as_ref().is_some_and(matches) {
            return Ok(());
        }
        if !self.flight.as_ref().is_some_and(matches) {
            return Err("Wrong Workspace terminal frame acknowledgement".into());
        }
        self.accepted = self.flight.take();
        Ok(())
    }

    fn input_for(&mut self, request: u32) -> Result<&mut WorkspaceTerminalAttachment, String> {
        if self.accepted.is_none() || request <= self.last_input_request {
            return Err("Stale Workspace input or initial frame unacknowledged".into());
        }
        self.last_input_request = request;
        self.connection
            .as_mut()
            .ok_or_else(|| "Workspace input lease is unavailable".into())
    }

    fn drain(&mut self) -> Result<(), String> {
        let Some(connection) = &mut self.connection else {
            return Ok(());
        };
        for _ in 0..EVENT_BUDGET {
            match connection.try_event()? {
                Some(WorkspaceTerminalEvent::Output(bytes)) => {
                    let reply = self.terminal.feed(&bytes)?;
                    if !reply.is_empty() {
                        connection.input_reply(reply)?;
                    }
                }
                Some(WorkspaceTerminalEvent::Exited { code }) => {
                    self.status = match code {
                        Some(code) => format!("Workspace provider exited with status {code}."),
                        None => "Workspace provider exited.".into(),
                    };
                    self.connection = None;
                    break;
                }
                None => break,
            }
        }
        Ok(())
    }

    fn poll(&mut self) -> Result<Polled, String> {
        if let Err(error) = self.drain() {
            self.connection = None;
            self.status = error;
        }
        let mut frame = None;
        if self.flight.is_none() && (self.connection.is_none() || !self.terminal.publication_held())
        {
            let latest = self.terminal.frame()?;
            if !self
                .accepted
                .as_ref()
                .is_some_and(|last| last.stamp() == latest.stamp())
            {
                let owner = &self.receipt.identity.owner;
                frame = Some(semantic::update(
                    self.accepted.as_ref(),
                    &latest,
                    &owner.session,
                    owner.epoch,
                )?);
                self.flight = Some(latest);
            }
        }
        Ok(Polled {
            frame,
            writable: self.connection.is_some() && self.accepted.is_some(),
            status: self.status.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_identity_and_initial_frame_ack_gate_workspace_input() {
        let identity = Identity {
            conversation: ConversationId("fixture".into()),
            owner: Attachment {
                session: "a".repeat(32),
                epoch: 2,
                generation: 2,
            },
        };
        let terminal = Terminal::new(2, 3).unwrap();
        let frame = terminal.frame().unwrap();
        let mut views = Views {
            active: Some(View {
                opening_request: 1,
                receipt: Attached {
                    identity: identity.clone(),
                    frame: tauri_terminal_frame_bridge::from_engine(
                        &frame,
                        &identity.owner.session,
                        2,
                    )
                    .unwrap(),
                },
                connection: None,
                terminal,
                accepted: None,
                flight: Some(frame),
                status: String::new(),
                last_input_request: 0,
            }),
            generation: 2,
            last_detached: None,
        };
        let text = || Operation::Text { text: "x".into() };
        let mut stale = identity.clone();
        stale.owner.generation = 1;
        assert!(
            matches!(views.execute(2, &stale, text()), Err(error) if error.contains("Stale Workspace terminal attachment"))
        );
        assert!(
            matches!(views.execute(3, &identity, text()), Err(error) if error.contains("initial frame"))
        );
        assert!(views
            .execute(4, &identity, Operation::Ack { revision: 2 })
            .is_err());
        assert!(views
            .execute(5, &identity, Operation::Ack { revision: 1 })
            .is_ok());
        assert!(
            matches!(views.execute(6, &identity, text()), Err(error) if error.contains("lease is unavailable"))
        );
        assert!(views.execute(7, &identity, Operation::Detach).is_ok());
        assert!(views.execute(8, &identity, Operation::Detach).is_ok());
        assert!(views.execute(9, &identity, Operation::Poll).is_err());
    }
}
