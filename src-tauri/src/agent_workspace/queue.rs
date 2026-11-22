//! Native view over the shared Workspace dependency queue and its typed receipts.
use super::native_tasks;
use jterm_core::agent_workspace::{
    ConversationId, ConversationKind, WorkspaceClient, WorkspaceCommand as Command,
    WorkspaceExecutionLocation, WorkspaceQueueChange, WorkspaceQueueIntent, WorkspaceQueueSnapshot,
    WorkspaceQueueState, WorkspaceQueueUpdate, WorkspaceReply,
};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, tag = "operation", rename_all = "snake_case")]
pub(crate) enum Choice {
    Read,
    Save {
        source: native_tasks::Identity,
        revision: String,
        dependencies: Vec<ConversationId>,
    },
    Remove {
        source: native_tasks::Identity,
        revision: String,
    },
}

impl Choice {
    pub(super) fn source(&self) -> Option<&native_tasks::Identity> {
        match self {
            Self::Read => None,
            Self::Save { source, .. } | Self::Remove { source, .. } => Some(source),
        }
    }
}

#[derive(serde::Serialize)]
pub(crate) struct View {
    revision: String,
    rows: Vec<Row>,
    tasks: Vec<Task>,
}

#[derive(serde::Serialize)]
struct Row {
    id: ConversationId,
    title: String,
    state: &'static str,
    ready: bool,
    reason: Option<String>,
    location: String,
    dependencies: Vec<ConversationId>,
}

#[derive(serde::Serialize)]
struct Task {
    id: ConversationId,
    title: String,
}

#[derive(Default)]
pub(super) struct Queue {
    client: Option<WorkspaceClient>,
    observed: Option<u64>,
}

impl Queue {
    pub(super) fn execute(
        &mut self,
        choice: Choice,
        source: Option<ConversationId>,
    ) -> Result<View, String> {
        let current = self.read()?;
        let change = match choice {
            Choice::Read => return self.project(current),
            Choice::Save {
                revision,
                dependencies,
                ..
            } => {
                self.require_current(&revision, &current)?;
                let id =
                    source.ok_or("Select the exact native task before saving its queue plan")?;
                let location = current
                    .entries
                    .iter()
                    .find(|entry| entry.intent.id == id)
                    .map_or(WorkspaceExecutionLocation::Local, |entry| {
                        entry.intent.location.clone()
                    });
                let intent = WorkspaceQueueIntent {
                    id,
                    dependencies,
                    location,
                };
                intent.validate()?;
                WorkspaceQueueChange::Upsert { intent }
            }
            Choice::Remove { revision, .. } => {
                self.require_current(&revision, &current)?;
                let id =
                    source.ok_or("Select the exact native task before removing its queue plan")?;
                WorkspaceQueueChange::Remove { id }
            }
        };

        let expected = current
            .revision
            .checked_add(1)
            .ok_or("Task queue revision exhausted")?;
        let WorkspaceReply::Queue { snapshot } = super::request_host(
            &mut self.client,
            Command::QueueUpdate {
                update: WorkspaceQueueUpdate {
                    expected_revision: current.revision,
                    change,
                },
            },
        )?
        else {
            return Err("Missing shared queue update receipt".into());
        };
        if snapshot.revision != expected {
            return Err("Unexpected shared queue revision".into());
        }
        self.project(snapshot).map_err(|error| {
            format!("Queue revision {expected} was saved. Refresh to read readiness: {error}")
        })
    }

    fn require_current(
        &self,
        revision: &str,
        snapshot: &WorkspaceQueueSnapshot,
    ) -> Result<(), String> {
        if self.observed != Some(snapshot.revision) || snapshot.revision.to_string() != revision {
            return Err("The task queue changed in another view; refresh before retrying".into());
        }
        Ok(())
    }

    fn read(&mut self) -> Result<WorkspaceQueueSnapshot, String> {
        match super::request_host(&mut self.client, Command::QueueSnapshot)? {
            WorkspaceReply::Queue { snapshot } => Ok(snapshot),
            _ => Err("Missing shared queue snapshot".into()),
        }
    }

    fn project(&mut self, snapshot: WorkspaceQueueSnapshot) -> Result<View, String> {
        let WorkspaceReply::Catalog { rows } =
            super::request_host(&mut self.client, Command::Refresh)?
        else {
            return Err("Missing Workspace task catalog".into());
        };
        let complete = rows.len() < 4096;
        let queue_rows = snapshot
            .project(&rows)
            .into_iter()
            .zip(snapshot.entries.iter())
            .map(|(row, entry)| {
                let verified = row.state != WorkspaceQueueState::Ready || complete;
                Row {
                    id: row.id,
                    title: row.title,
                    state: if verified {
                        row.state.label()
                    } else {
                        "Readiness unconfirmed"
                    },
                    ready: complete && row.state == WorkspaceQueueState::Ready,
                    reason: row.reason,
                    location: row.location.label(),
                    dependencies: entry.intent.dependencies.clone(),
                }
            })
            .collect();
        let tasks = rows
            .into_iter()
            .filter(|row| row.kind == ConversationKind::Task && !row.archived)
            .map(|row| Task {
                id: row.id,
                title: row.title,
            })
            .collect();
        self.observed = Some(snapshot.revision);
        Ok(View {
            rows: queue_rows,
            tasks,
            revision: snapshot.revision.to_string(),
        })
    }
}
