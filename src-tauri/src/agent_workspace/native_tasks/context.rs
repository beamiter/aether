//! Exact native-task context over the shared revisioned Workspace store.
use super::{number, Identity, Reply};
use jterm_core::{
    agent_task::TaskBrief,
    agent_workspace::{
        MaterialKind, WorkspaceClient, WorkspaceCommand as Command, WorkspaceContext,
        WorkspaceHandoff, WorkspaceMaterial, WorkspaceReply,
    },
};
use uuid::Uuid;

const TARGET: &str = "codex";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub(crate) enum Choice {
    Read,
    Save {
        revision: String,
        fields: Fields,
        note: Option<String>,
    },
    Preview {
        revision: String,
    },
    Use {
        revision: String,
    },
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fields {
    goal: String,
    completed: String,
    remaining: String,
    constraints: String,
}

#[derive(serde::Serialize)]
pub(crate) struct ContextView {
    identity: Identity,
    revision: String,
    fields: Fields,
    materials: Vec<WorkspaceMaterial>,
}

#[derive(serde::Serialize)]
pub(crate) struct HandoffView {
    identity: Identity,
    revision: String,
    title: String,
    repository: String,
    body: String,
}

pub(super) fn execute(
    client: &mut Option<WorkspaceClient>,
    identity: &Identity,
    preview: &mut Option<WorkspaceHandoff>,
    choice: Choice,
) -> Result<Reply, String> {
    match choice {
        Choice::Read => {
            *preview = None;
            let value = read(client, identity)?;
            Ok(project(identity, value))
        }
        Choice::Save {
            revision,
            fields,
            note,
        } => {
            *preview = None;
            let revision = number(&revision)?;
            let mut context = read(client, identity)?;
            if context.revision != revision {
                return Err("Shared context changed. Your edits are still in the form.".into());
            }
            context.goal = fields.goal;
            context.completed = fields.completed;
            context.remaining = fields.remaining;
            context.constraints = fields.constraints;
            if let Some(text) = note {
                if text.trim().is_empty() {
                    return Err("Enter a note before adding it to the context".into());
                }
                context.materials.push(WorkspaceMaterial {
                    id: Uuid::new_v4().to_string(),
                    label: "Task note".into(),
                    kind: MaterialKind::Note,
                    text,
                    source: None,
                    truncated: false,
                });
            }
            context.validate()?;
            let expected = revision
                .checked_add(1)
                .ok_or("Context revision is exhausted")?;
            let WorkspaceReply::Context { context } = request(
                client,
                Command::SaveContext {
                    id: identity.task.clone(),
                    expected_revision: revision,
                    context,
                },
            )?
            else {
                return Err("Missing shared context save receipt".into());
            };
            if context.conversation != identity.task || context.revision != expected {
                return Err("Unexpected shared context save receipt".into());
            }
            Ok(project(identity, context))
        }
        Choice::Preview { revision } => {
            *preview = None;
            let packet = build(client, identity, number(&revision)?)?;
            let receipt = handoff_view(identity, &packet);
            *preview = Some(packet);
            Ok(Reply::ContextPreview {
                preview: Box::new(receipt),
            })
        }
        Choice::Use { revision } => {
            let revision = number(&revision)?;
            let reviewed = preview
                .take()
                .filter(|packet| packet.source_revision == revision)
                .ok_or("Preview the current context before opening a new draft")?;
            let packet = build(client, identity, revision)?;
            if reviewed != packet {
                return Err("The handoff changed; preview it again before opening a draft".into());
            }
            Ok(Reply::ContextDraft {
                draft: Box::new(handoff_view(identity, &packet)),
            })
        }
    }
}

fn read(
    client: &mut Option<WorkspaceClient>,
    identity: &Identity,
) -> Result<WorkspaceContext, String> {
    match request(
        client,
        Command::ReadContext {
            id: identity.task.clone(),
        },
    )? {
        WorkspaceReply::Context { context } if context.conversation == identity.task => Ok(context),
        _ => Err("Missing shared context receipt for this task".into()),
    }
}

fn project(identity: &Identity, context: WorkspaceContext) -> Reply {
    Reply::Context {
        context: Box::new(ContextView {
            identity: identity.clone(),
            revision: context.revision.to_string(),
            fields: Fields {
                goal: context.goal,
                completed: context.completed,
                remaining: context.remaining,
                constraints: context.constraints,
            },
            materials: context.materials,
        }),
    }
}

fn build(
    client: &mut Option<WorkspaceClient>,
    identity: &Identity,
    revision: u64,
) -> Result<WorkspaceHandoff, String> {
    let WorkspaceReply::Handoff { packet } = request(
        client,
        Command::BuildHandoff {
            id: identity.task.clone(),
            expected_revision: revision,
            target_provider: TARGET.into(),
        },
    )?
    else {
        return Err("Missing shared handoff receipt".into());
    };
    if packet.source != identity.task
        || packet.source_revision != revision
        || packet.target_provider != TARGET
    {
        return Err("Unexpected shared handoff source or revision".into());
    }
    TaskBrief::new(&packet.body, None)?;
    Ok(packet)
}

fn handoff_view(identity: &Identity, packet: &WorkspaceHandoff) -> HandoffView {
    HandoffView {
        identity: identity.clone(),
        revision: packet.source_revision.to_string(),
        title: packet.title.clone(),
        repository: packet.cwd.clone(),
        body: packet.body.clone(),
    }
}

fn request(
    client: &mut Option<WorkspaceClient>,
    command: Command,
) -> Result<WorkspaceReply, String> {
    super::super::request_host(client, command)
}
