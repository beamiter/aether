//! View state for host-owned API conversations. SQLite and providers stay in core.
use jterm_core::{
    agent_workspace::{
        ApiControlLease, ApiLaunch, ApiSnapshot, ApiState, ConversationId, ConversationKind,
        HostedApiClient, StoredChat, WorkspaceClient, WorkspaceCommand as Command, WorkspaceReply,
    },
    ai::{
        build_system_prompt, user_prompt_with_block_context, AiClient, ConversationSnapshot, Role,
        Turn, DEFAULT_CHAT_TITLE,
    },
};

#[derive(Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    conversation: ConversationId,
    generation: u32,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, tag = "operation", rename_all = "snake_case")]
pub(crate) enum Action {
    List {
        before: Option<String>,
    },
    Create,
    Open {
        conversation: ConversationId,
    },
    Reload {
        identity: Identity,
    },
    Save {
        identity: Identity,
        revision: String,
        text: String,
    },
    Send {
        identity: Identity,
        revision: String,
        draft_revision: String,
        text: String,
    },
    Observe {
        identity: Identity,
    },
    Cancel {
        identity: Identity,
    },
    Detach {
        identity: Identity,
    },
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Reply {
    Catalog {
        rows: Vec<Row>,
        next_before: Option<String>,
    },
    Chat {
        chat: Box<Chat>,
    },
    Job {
        identity: Identity,
        job: Job,
    },
    Ack,
}

#[derive(serde::Serialize)]
pub(crate) struct Row {
    id: ConversationId,
    title: String,
}

#[derive(serde::Serialize)]
pub(crate) struct Chat {
    identity: Identity,
    title: String,
    archived: bool,
    background_supported: bool,
    terminal_context: bool,
    revision: String,
    draft_revision: String,
    draft: String,
    turns: Vec<Turn>,
    has_older: bool,
    job: Option<Job>,
}

#[derive(serde::Serialize)]
pub(crate) struct Job {
    epoch: String,
    sequence: String,
    state: ApiState,
    user_text: String,
    preview: String,
    can_cancel: bool,
}

struct View {
    identity: Identity,
    epoch: Option<u64>,
    sequence: Option<u64>,
    control: Option<ApiControlLease>,
}

#[derive(Default)]
pub(super) struct Chats {
    client: Option<WorkspaceClient>,
    generation: u32,
    active: Option<View>,
}

impl Chats {
    pub(super) fn execute(&mut self, action: Action) -> Result<Reply, String> {
        match action {
            Action::List { before } => self.list(before),
            Action::Create => {
                self.available()?;
                let chat = self.chat(Command::CreateChat {
                    title: DEFAULT_CHAT_TITLE.into(),
                })?;
                self.open(chat)
            }
            Action::Open { conversation } => {
                self.available()?;
                let chat = self.load(&conversation)?;
                self.open(chat)
            }
            Action::Reload { identity } => {
                self.view(&identity)?;
                let chat = self.load(&identity.conversation)?;
                self.project(chat)
            }
            Action::Save {
                identity,
                revision,
                text,
            } => {
                self.view(&identity)?;
                let chat = self.chat(Command::SetDraft {
                    id: identity.conversation,
                    expected_revision: number(&revision)?,
                    text,
                })?;
                self.project(chat)
            }
            Action::Send {
                identity,
                revision,
                draft_revision,
                text,
            } => {
                self.view(&identity)?;
                self.submit(identity, number(&revision)?, number(&draft_revision)?, text)
            }
            Action::Observe { identity } => {
                self.view(&identity)?;
                let (snapshot, _) = self.inspect(&identity.conversation)?;
                let job = self.view_mut()?.adopt(snapshot)?;
                Ok(Reply::Job { identity, job })
            }
            Action::Cancel { identity } => {
                let control = self
                    .view(&identity)?
                    .control
                    .clone()
                    .ok_or("This view does not own API control")?;
                acknowledge(self.request(Command::CancelApi { control })?)?;
                let (snapshot, _) = self.inspect(&identity.conversation)?;
                let job = self.view_mut()?.adopt(snapshot)?;
                Ok(Reply::Job { identity, job })
            }
            Action::Detach { identity } => self.detach(&identity),
        }
    }

    fn available(&self) -> Result<(), String> {
        if self.active.is_some() {
            return Err("Detach the current API conversation first".into());
        }
        Ok(())
    }

    fn list(&mut self, before: Option<String>) -> Result<Reply, String> {
        let before = before.as_deref().map(number).transpose()?;
        let WorkspaceReply::CatalogPage {
            rows, next_before, ..
        } = self.request(Command::CatalogPage { before, limit: 32 })?
        else {
            return Err("Unexpected response from the shared Workspace catalog".into());
        };
        let rows = rows
            .into_iter()
            .filter(|row| row.kind == ConversationKind::Api && !row.archived)
            .map(|row| Row {
                id: row.id,
                title: row.title,
            })
            .collect();
        Ok(Reply::Catalog {
            rows,
            next_before: next_before.map(|v| v.to_string()),
        })
    }

    fn open(&mut self, chat: StoredChat) -> Result<Reply, String> {
        // Parse the bounded authoritative snapshot before acquiring control.
        let view = self.describe(&chat)?;
        let generation = self
            .generation
            .checked_add(1)
            .ok_or("API view generation exhausted")?;
        let identity = Identity {
            conversation: chat.row.id.clone(),
            generation,
        };
        let mut active = View {
            identity,
            epoch: None,
            sequence: None,
            control: None,
        };
        let job = if let Some(original) = chat.api_job.clone().map(|job| *job) {
            let (snapshot, control) = if original.state.is_live() {
                match self
                    .request(Command::AttachApi {
                        id: active.identity.conversation.clone(),
                        epoch: original.epoch,
                        view: format!("aether-{generation}"),
                    })
                    .and_then(api_reply)
                {
                    Ok(reply) => reply,
                    Err(_) => self.inspect(&active.identity.conversation)?,
                }
            } else {
                (original.clone(), None)
            };
            if snapshot.epoch != original.epoch || snapshot.id != active.identity.conversation {
                return Err("API execution changed while opening; reopen the conversation".into());
            }
            active.control = control;
            Some(active.adopt(snapshot)?)
        } else {
            None
        };
        let reply = self.combine(&active, chat, view, job);
        self.active = Some(active);
        self.generation = generation;
        Ok(reply)
    }

    fn submit(
        &mut self,
        identity: Identity,
        revision: u64,
        draft_revision: u64,
        text: String,
    ) -> Result<Reply, String> {
        let current = self.load(&identity.conversation)?;
        if !current.background_api_supported {
            return Err("Update the Workspace host before submitting API conversations".into());
        }
        let snapshot = self.describe(&current)?;
        let draft = snapshot.active_chat().ok_or("Missing saved draft")?.draft();
        if current.revision != revision || current.draft_revision != draft_revision || draft != text
        {
            return Err(
                "The exact draft revision has not been acknowledged by the Workspace host".into(),
            );
        }
        if std::env::var_os("AETHER_AI_PROVIDER").is_none_or(|provider| provider.is_empty()) {
            return Err("Configure AETHER_AI_PROVIDER before sending an API conversation".into());
        }
        let client = AiClient::from_env()
            .map_err(|_| "Aether API provider configuration is unavailable or invalid")?;
        let mut turns = request_history(snapshot.turns());
        let context = snapshot.block_context();
        let system = context.and_then(|block| build_system_prompt(Some(block)));
        turns.push(Turn {
            role: Role::User,
            text: context.map_or_else(
                || text.clone(),
                |block| user_prompt_with_block_context(&text, Some(block)),
            ),
        });
        let WorkspaceReply::Submission { receipt } = self.request(Command::BeginSubmission {
            id: identity.conversation.clone(),
            expected_revision: revision,
            draft_revision,
            text,
        })?
        else {
            return Err("Missing Workspace submission receipt".into());
        };
        if receipt.id != identity.conversation || receipt.draft_revision != draft_revision {
            return Err("Workspace submission receipt belongs to another draft".into());
        }
        let launch = match ApiLaunch::new(receipt.clone(), &client, system, &turns, true) {
            Ok(launch) => launch,
            Err(error) => {
                let _ = self.request(Command::CancelSubmission {
                    receipt,
                    restore_draft: true,
                });
                return Err(error);
            }
        };
        let (snapshot, control) = HostedApiClient::new().start_confirmed(launch)?;
        let view = self.view_mut()?;
        view.epoch = Some(receipt.epoch);
        view.sequence = None;
        view.control = control;
        let job = view.adopt(snapshot)?;
        Ok(Reply::Job { identity, job })
    }

    fn detach(&mut self, identity: &Identity) -> Result<Reply, String> {
        let view = self.view(identity)?;
        if let Some(control) = view.control.clone() {
            if let Err(error) = self
                .request(Command::DetachApi {
                    control: control.clone(),
                })
                .and_then(acknowledge)
            {
                let (snapshot, _) = self.inspect(&control.id)?;
                if snapshot.epoch == control.epoch && snapshot.state.is_live() {
                    return Err(error);
                }
            }
        }
        self.active = None;
        Ok(Reply::Ack)
    }

    fn describe(&self, chat: &StoredChat) -> Result<ConversationSnapshot, String> {
        ConversationSnapshot::from_json(&chat.snapshot_json).map_err(|e| e.to_string())
    }

    fn combine(
        &self,
        view: &View,
        chat: StoredChat,
        decoded: ConversationSnapshot,
        job: Option<Job>,
    ) -> Reply {
        let draft = decoded
            .active_chat()
            .map(|chat| chat.draft().to_owned())
            .unwrap_or_default();
        Reply::Chat {
            chat: Box::new(Chat {
                identity: view.identity.clone(),
                title: chat.row.title,
                archived: chat.row.archived,
                background_supported: chat.background_api_supported,
                terminal_context: decoded.block_context().is_some(),
                revision: chat.revision.to_string(),
                draft_revision: chat.draft_revision.to_string(),
                draft,
                turns: decoded.turns().to_vec(),
                has_older: chat.has_older,
                job,
            }),
        }
    }

    fn project(&mut self, chat: StoredChat) -> Result<Reply, String> {
        let decoded = self.describe(&chat)?;
        let view = self.view_mut()?;
        if view.identity.conversation != chat.row.id {
            return Err("Workspace conversation identity changed".into());
        }
        let job = chat
            .api_job
            .clone()
            .map(|snapshot| view.adopt(*snapshot))
            .transpose()?;
        let view = self.active.as_ref().expect("selected API view");
        Ok(self.combine(view, chat, decoded, job))
    }

    fn inspect(
        &mut self,
        id: &ConversationId,
    ) -> Result<(ApiSnapshot, Option<ApiControlLease>), String> {
        api_reply(self.request(Command::InspectApi { id: id.clone() })?)
    }

    fn load(&mut self, id: &ConversationId) -> Result<StoredChat, String> {
        let chat = self.chat(Command::LoadChat { id: id.clone() })?;
        if &chat.row.id != id {
            return Err("Workspace conversation identity changed".into());
        }
        Ok(chat)
    }

    fn chat(&mut self, command: Command) -> Result<StoredChat, String> {
        match self.request(command)? {
            WorkspaceReply::Chat { chat } if chat.row.kind == ConversationKind::Api => Ok(chat),
            _ => Err("Unexpected response from the shared API conversation store".into()),
        }
    }

    fn view(&self, identity: &Identity) -> Result<&View, String> {
        self.active
            .as_ref()
            .filter(|view| view.identity == *identity)
            .ok_or_else(|| "Stale API conversation view".into())
    }

    fn view_mut(&mut self) -> Result<&mut View, String> {
        self.active
            .as_mut()
            .ok_or_else(|| "Select an API conversation".into())
    }

    fn request(&mut self, command: Command) -> Result<WorkspaceReply, String> {
        super::request_host(&mut self.client, command)
    }
}

impl View {
    fn adopt(&mut self, snapshot: ApiSnapshot) -> Result<Job, String> {
        if snapshot.id != self.identity.conversation
            || self.epoch.is_some_and(|epoch| epoch > snapshot.epoch)
            || (self.epoch == Some(snapshot.epoch)
                && self.sequence.is_some_and(|seq| seq > snapshot.sequence))
        {
            return Err("Stale or foreign API execution snapshot".into());
        }
        self.epoch = Some(snapshot.epoch);
        self.sequence = Some(snapshot.sequence);
        if !snapshot.state.is_live()
            || self
                .control
                .as_ref()
                .is_some_and(|control| control.epoch != snapshot.epoch || control.id != snapshot.id)
        {
            self.control = None;
        }
        Ok(Job {
            epoch: snapshot.epoch.to_string(),
            sequence: snapshot.sequence.to_string(),
            state: snapshot.state,
            user_text: snapshot.user_text,
            preview: snapshot.preview,
            can_cancel: self.control.is_some()
                && matches!(snapshot.state, ApiState::Starting | ApiState::Running),
        })
    }
}

fn request_history(turns: &[Turn]) -> Vec<Turn> {
    let mut budget = 1024 * 1024;
    let mut count = 0;
    for pair in turns.rchunks_exact(2).take(24) {
        let bytes = pair[0].text.len() + pair[1].text.len();
        if bytes > budget {
            break;
        }
        budget -= bytes;
        count += 2;
    }
    turns[turns.len() - count..].to_vec()
}

fn number(value: &str) -> Result<u64, String> {
    if value.len() > 20 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Invalid Workspace revision".into());
    }
    value
        .parse()
        .map_err(|_| "Invalid Workspace revision".into())
}

fn api_reply(reply: WorkspaceReply) -> Result<(ApiSnapshot, Option<ApiControlLease>), String> {
    match reply {
        WorkspaceReply::Api { snapshot, control } => Ok((snapshot, control)),
        _ => Err("Unexpected Workspace API observation response".into()),
    }
}

fn acknowledge(reply: WorkspaceReply) -> Result<(), String> {
    match reply {
        WorkspaceReply::Ack => Ok(()),
        _ => Err("Missing Workspace acknowledgement".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_api_sequence_and_control_are_owned_by_the_current_epoch() {
        let id = ConversationId("jterm:api:fixture".into());
        let mut view = View {
            identity: Identity {
                conversation: id.clone(),
                generation: 2,
            },
            epoch: Some(4),
            sequence: Some(10),
            control: Some(ApiControlLease {
                id: id.clone(),
                epoch: 4,
                generation: "fixture".into(),
            }),
        };
        let snapshot = ApiSnapshot {
            id,
            epoch: 4,
            sequence: 11,
            state: ApiState::Running,
            user_text: String::new(),
            submitted_draft_revision: 6,
            preview: "first".into(),
            result: None,
            error: None,
        };
        assert!(view.adopt(snapshot.clone()).unwrap().can_cancel);
        assert!(view
            .adopt(ApiSnapshot {
                sequence: 9,
                ..snapshot.clone()
            })
            .is_err());
        assert!(view
            .adopt(ApiSnapshot {
                epoch: 3,
                ..snapshot.clone()
            })
            .is_err());
        assert!(
            !view
                .adopt(ApiSnapshot {
                    state: ApiState::Completed,
                    sequence: 12,
                    ..snapshot
                })
                .unwrap()
                .can_cancel
        );
        assert!(view.control.is_none());
    }
}
