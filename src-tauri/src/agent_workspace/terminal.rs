pub(crate) use jterm_core::agent_workspace::{
    WorkspaceViewportOperation as Operation, WorkspaceViewportReply as Reply,
};
use jterm_core::{
    agent_sessions::AgentSession,
    agent_workspace::{ConversationId, WorkspaceViewportAttachment, WorkspaceViewportPoll},
};
use tauri_terminal_frame_bridge::{semantic::Attachment, WireFrame};

const ROWS: u16 = 24;
const COLS: u16 = 80;

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

#[derive(Default)]
pub(super) struct Views {
    generation: u32,
    active: Option<View>,
    last_detached: Option<Identity>,
}

struct View {
    opening_request: u32,
    receipt: Attached,
    connection: Option<WorkspaceViewportAttachment>,
    accepted: Option<u32>,
    flight: Option<u32>,
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
        let (connection, snapshot, frame) =
            WorkspaceViewportAttachment::attach(source, ROWS, COLS, generation)?;
        if snapshot.id != conversation {
            return Err("Workspace host returned a different conversation".into());
        }
        let owner = Attachment {
            session: frame.session.clone(),
            epoch: frame.epoch,
            generation,
        };
        let revision = frame.revision;
        let receipt = Attached {
            frame,
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
            accepted: None,
            flight: Some(revision),
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
                if let Some(mut connection) = view.connection.take() {
                    let _ = connection.execute(Operation::Detach);
                }
                self.last_detached = Some(identity.clone());
                self.active = None;
                Ok(Reply::Receipt)
            }
            Operation::Poll => view.poll(),
            Operation::Ack { revision } => view.ack(revision),
            input => view.input_for(request)?.execute(input),
        }
    }
}

impl View {
    fn ack(&mut self, revision: u32) -> Result<Reply, String> {
        if self.flight.is_none() && self.accepted == Some(revision) {
            return Ok(Reply::Receipt);
        }
        if self.flight != Some(revision) {
            return Err("Wrong Workspace terminal frame acknowledgement".into());
        }
        if let Some(connection) = &mut self.connection {
            if !matches!(
                connection.execute(Operation::Ack { revision })?,
                Reply::Receipt
            ) {
                return Err("Session Host did not acknowledge the viewport frame".into());
            }
        }
        self.accepted = self.flight.take();
        Ok(Reply::Receipt)
    }

    fn input_for(&mut self, request: u32) -> Result<&mut WorkspaceViewportAttachment, String> {
        if self.accepted.is_none() || request <= self.last_input_request {
            return Err("Stale Workspace input or initial frame unacknowledged".into());
        }
        self.last_input_request = request;
        self.connection
            .as_mut()
            .ok_or_else(|| "Workspace input lease is unavailable".into())
    }

    fn poll(&mut self) -> Result<Reply, String> {
        let result = self
            .connection
            .as_mut()
            .ok_or_else(|| "Workspace viewport lease is unavailable".to_string())
            .and_then(|connection| connection.execute(Operation::Poll));
        match result {
            Ok(Reply::Polled { update }) => {
                if let Some(frame) = &update.frame {
                    self.flight = Some(frame.revision());
                }
                Ok(Reply::Polled { update })
            }
            Ok(_) => Err("Session Host returned an invalid viewport update".into()),
            Err(status) => {
                self.connection = None;
                Ok(Reply::Polled {
                    update: WorkspaceViewportPoll {
                        frame: None,
                        writable: false,
                        status,
                        finished: true,
                    },
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jterm_engine::terminal::Terminal;

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
                accepted: None,
                flight: Some(1),
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
