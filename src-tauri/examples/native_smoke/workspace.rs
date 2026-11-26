use crate::application::Workspace;
use jterm_core::agent_workspace::{
    task_id, ConversationKind, RuntimeState, SearchSource, WorkspaceClient, WorkspaceCommand,
    WorkspaceEvent, WorkspaceReply, WorkspaceRow, WorkspaceTextDocument,
};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CLI_ID: &str = "018f418b-1f89-7bbd-b9cd-a0d50d0f0312";

pub(super) struct Fixture {
    root: PathBuf,
    host: Option<Child>,
}

impl Fixture {
    pub(super) fn start() -> Result<Self, String> {
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?;
        let root = PathBuf::from("/tmp").join(format!(
            "aether-smoke-{}-{}",
            std::process::id(),
            created.as_nanos(),
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .map_err(|error| error.to_string())?;
        let mut fixture = Self { root, host: None };
        fixture.seed()?;
        Ok(fixture)
    }

    pub(super) fn workspace(&self) -> Workspace {
        Workspace::from_paths(
            Some(self.root.join("codex")),
            Some(self.root.join("claude")),
            Some(self.root.join("state")),
        )
    }

    fn provider_source(&self) -> PathBuf {
        self.root.join(format!(
            "codex/sessions/rollout-2026-11-20T01-02-03-{CLI_ID}.jsonl"
        ))
    }

    fn provider_transcript(&self) -> String {
        let metadata = serde_json::json!({"type": "session_meta", "payload": {
            "id": CLI_ID, "cwd": self.root.join("project"),
        }});
        let title = serde_json::json!({"type": "event_msg", "payload": {
            "type": "user_message", "message": "Aether CLI fixture",
        }});
        let done = serde_json::json!({"type": "event_msg", "payload": {"type": "task_complete"}});
        format!("{metadata}\n{title}\n{done}\n")
    }

    pub(super) fn verify_provider_source(&self) -> Result<(), String> {
        let current =
            fs::read_to_string(self.provider_source()).map_err(|error| error.to_string())?;
        if current == self.provider_transcript() {
            Ok(())
        } else {
            Err("Saved CLI deletion changed the original provider transcript".into())
        }
    }

    fn seed(&mut self) -> Result<(), String> {
        let project = self.root.join("project");
        fs::create_dir(&project).map_err(|error| error.to_string())?;
        fs::create_dir_all(self.root.join("codex/sessions")).map_err(|error| error.to_string())?;
        fs::write(self.provider_source(), self.provider_transcript())
            .map_err(|error| error.to_string())?;

        let state = self.root.join("state");
        let binary = std::env::current_exe().map_err(|error| error.to_string())?;
        let host = Command::new(binary)
            .arg("--jterm-session-host")
            .env("JTERM_WORKSPACE_STATE_DIR", &state)
            .stdin(Stdio::null())
            .spawn()
            .map_err(|error| error.to_string())?;
        self.host = Some(host);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !state.join("run/host.sock").exists() {
            if Instant::now() >= deadline {
                return Err("Temporary Workspace host failed to start".into());
            }
            thread::sleep(Duration::from_millis(20));
        }

        let client = WorkspaceClient::for_state_dir(state);
        request(
            &client,
            WorkspaceCommand::IndexSessions {
                sessions: vec![WorkspaceRow {
                    id: task_id("aether-smoke", "fixture")?,
                    kind: ConversationKind::Task,
                    title: "Aether archived task fixture".into(),
                    provider: "Codex".into(),
                    cwd: project.to_string_lossy().into_owned(),
                    preview: String::new(),
                    updated_at_ms: 1,
                    archived: true,
                    pinned: false,
                    tags: Vec::new(),
                    runtime: RuntimeState::Idle,
                    facts: Default::default(),
                }],
            },
        )?;
        let WorkspaceReply::Chat { chat } = request(
            &client,
            WorkspaceCommand::CreateChat {
                title: "Aether API fixture".into(),
            },
        )?
        else {
            return Err("Temporary Workspace host did not create the fixture chat".into());
        };
        let id = chat.row.id;
        request(
            &client,
            WorkspaceCommand::Pin {
                id: id.clone(),
                pinned: true,
            },
        )?;
        request(
            &client,
            WorkspaceCommand::IndexText {
                documents: vec![WorkspaceTextDocument {
                    conversation: id,
                    event_id: "external:aether-native-smoke".into(),
                    revision: "fixture-v1".into(),
                    source: SearchSource::ImportedHistory,
                    message_sequence: None,
                    text: "aether-needle <b>literal evidence</b>".into(),
                    truncated: false,
                }],
            },
        )?;
        Ok(())
    }
}

fn request(client: &WorkspaceClient, command: WorkspaceCommand) -> Result<WorkspaceReply, String> {
    let expected = client.send(command)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(WorkspaceEvent::Reply { request_id, result }) = client.poll().pop() {
            if request_id == expected {
                return result;
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err("Temporary Workspace host request timed out".into())
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(host) = &mut self.host {
            let _ = host.kill();
            let _ = host.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
