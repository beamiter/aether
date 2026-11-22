//! Exact native task and approval views over the shared Workspace owner.
use jterm_core::{
    agent_task::{
        AgentProvider, ApprovalDecision, ApprovalId, CodexAppServerApproval, CreateWorktreeRequest,
        NativePromptPolicy, NewTask, TaskBrief, TaskManager, TaskStatus, TaskValidationRecipe,
        TaskValidationStatus, WorktreeService,
    },
    agent_workspace::{
        native_worktrees_directory, task_id, ConversationId, NativeAction, NativeControlLease,
        NativeDraft, NativeLaunch, NativeLaunchContext, NativeSnapshot, NativeTaskMutation,
        WorkspaceClient, WorkspaceCommand as Command, WorkspaceReply,
    },
};
use std::path::PathBuf;
use uuid::Uuid;

mod context;

const NAMESPACE: &str = "aether:workspace-native";
const PAGE_ROWS: usize = 24;
const VERIFICATION_SHELL: &str = "/bin/sh";

#[derive(Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    task: ConversationId,
    incarnation: String,
    generation: u32,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, tag = "operation", rename_all = "snake_case")]
pub(crate) enum Action {
    List {
        before: Option<String>,
    },
    Start {
        title: String,
        repository: String,
        goal: String,
        consent: bool,
        verification: Option<String>,
    },
    SaveDraft {
        title: String,
        repository: String,
        goal: String,
        verification: Option<String>,
    },
    Open {
        task: ConversationId,
    },
    Observe {
        identity: Identity,
    },
    Decide {
        identity: Identity,
        sequence: String,
        operation_id: String,
        decision: Decision,
    },
    Result {
        identity: Identity,
    },
    Detach {
        identity: Identity,
    },
    Context {
        identity: Identity,
        choice: context::Choice,
    },
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub(crate) enum Decision {
    Decline {
        approval: ApprovalId,
    },
    Stop,
    Finish,
    Verify,
    AcceptVerified,
    AcceptManual,
    LaunchDraft {
        queue_revision: String,
        consent: bool,
    },
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Reply {
    Catalog {
        rows: Vec<Row>,
        next_before: Option<String>,
    },
    Task {
        task: Box<TaskView>,
    },
    Result {
        result: Box<ResultView>,
    },
    Context {
        context: Box<context::ContextView>,
    },
    ContextPreview {
        preview: Box<context::HandoffView>,
    },
    ContextDraft {
        draft: Box<context::HandoffView>,
    },
    Ack,
}

#[derive(serde::Serialize)]
pub(crate) struct Row {
    id: ConversationId,
    title: String,
}

#[derive(serde::Serialize)]
pub(crate) struct TaskView {
    identity: Identity,
    sequence: String,
    title: String,
    status: &'static str,
    worktree: PathBuf,
    branch: String,
    detail: Option<String>,
    output: String,
    truncated: bool,
    controlled: bool,
    can_stop: bool,
    can_finish: bool,
    can_inspect: bool,
    draft_goal: Option<String>,
    approvals: Vec<CodexAppServerApproval>,
    verification: VerificationView,
}

#[derive(serde::Serialize)]
struct VerificationView {
    recipe: Option<String>,
    shell: Option<String>,
    status: &'static str,
    output: String,
    truncated: bool,
    can_start: bool,
}

#[derive(serde::Serialize)]
pub(crate) struct ResultView {
    identity: Identity,
    sequence: String,
    status: String,
    diff: String,
    truncated: bool,
    validation: String,
    can_accept: bool,
    can_accept_review: bool,
    detail: Option<String>,
}

enum Review {
    Verified(u64),
    Manual { sequence: u64, fingerprint: String },
}

struct View {
    identity: Identity,
    token: String,
    sequence: u64,
    control: Option<NativeControlLease>,
    reviewed: Option<Review>,
    context_preview: Option<jterm_core::agent_workspace::WorkspaceHandoff>,
}

#[derive(PartialEq, Eq)]
struct Description {
    title: String,
    repository: String,
    goal: String,
    verification: Option<String>,
}

struct Prepared {
    description: Description,
    mode: Admission,
    launch: NativeLaunch,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Admission {
    Start,
    Draft,
}

#[derive(Default)]
pub(super) struct Tasks {
    client: Option<WorkspaceClient>,
    active: Option<View>,
    prepared: Option<Prepared>,
    generation: u32,
}

impl Tasks {
    pub(super) fn inspect_selected(
        &mut self,
        identity: &Identity,
    ) -> Result<NativeSnapshot, String> {
        self.selected(identity)?;
        let snapshot = self.inspect(&identity.task)?;
        if snapshot.incarnation != identity.incarnation {
            return Err("The selected native task run changed".into());
        }
        Ok(snapshot)
    }

    pub(super) fn execute(&mut self, action: Action) -> Result<Reply, String> {
        match action {
            Action::List { before } => self.list(before),
            Action::Start {
                title,
                repository,
                goal,
                consent,
                verification,
            } => {
                if !consent {
                    return Err(
                        "Confirm sharing the goal, optional verification command, and worktree with Codex".into(),
                    );
                }
                self.admit_task(
                    Description {
                        title,
                        repository,
                        goal,
                        verification,
                    },
                    Admission::Start,
                )
            }
            Action::SaveDraft {
                title,
                repository,
                goal,
                verification,
            } => self.admit_task(
                Description {
                    title,
                    repository,
                    goal,
                    verification,
                },
                Admission::Draft,
            ),
            Action::Open { task } => self.open(task),
            Action::Observe { identity } => {
                self.selected(&identity)?;
                let snapshot = self.inspect(&identity.task)?;
                self.project(snapshot)
            }
            Action::Decide {
                identity,
                sequence,
                operation_id,
                decision,
            } => self.decide(identity, sequence, operation_id, decision),
            Action::Result { identity } => self.result(identity),
            Action::Detach { identity } => self.detach(&identity),
            Action::Context { identity, choice } => {
                self.selected(&identity)?;
                if matches!(&choice, context::Choice::Use { .. }) && self.prepared.is_some() {
                    return Err(
                        "Resolve the previous task admission before opening a handoff".into(),
                    );
                }
                let view = self.active.as_mut().expect("selected task");
                let reply = context::execute(
                    &mut self.client,
                    &identity,
                    &mut view.context_preview,
                    choice,
                )?;
                if matches!(&reply, Reply::ContextDraft { .. }) {
                    self.detach(&identity)?;
                }
                Ok(reply)
            }
        }
    }

    fn detach(&mut self, identity: &Identity) -> Result<Reply, String> {
        let token = self.selected(identity)?.token.clone();
        match self.request(Command::DetachNativeView { view: token })? {
            WorkspaceReply::Ack => self.active = None,
            _ => return Err("Native task detach was not acknowledged".into()),
        }
        Ok(Reply::Ack)
    }

    fn list(&mut self, before: Option<String>) -> Result<Reply, String> {
        let before = before.as_deref().map(number).transpose()?;
        let WorkspaceReply::CatalogPage {
            rows, next_before, ..
        } = self.request(Command::CatalogPage {
            before,
            limit: PAGE_ROWS,
        })?
        else {
            return Err("Unexpected Workspace native task catalog".into());
        };
        Ok(Reply::Catalog {
            rows: rows
                .into_iter()
                .filter(|row| {
                    row.id.0.starts_with("jterm:task:") && row.facts.source_sequence.is_some()
                })
                .filter(|row| {
                    row.kind == jterm_core::agent_workspace::ConversationKind::Task && !row.archived
                })
                .map(|row| Row {
                    id: row.id,
                    title: row.title,
                })
                .collect(),
            next_before: next_before.map(|cursor| cursor.to_string()),
        })
    }

    fn available(&self) -> Result<(), String> {
        if self.active.is_some() {
            return Err("Detach the selected native task before opening another".into());
        }
        Ok(())
    }

    fn admit_task(&mut self, description: Description, mode: Admission) -> Result<Reply, String> {
        self.available()?;
        if self
            .prepared
            .as_ref()
            .is_some_and(|prepared| prepared.description != description || prepared.mode != mode)
        {
            return Err(
                "Retry the existing prepared task with its original fields and action".into(),
            );
        }
        if self.prepared.is_none() {
            // Verify the running host accepts native requests before creating a worktree.
            if !matches!(
                self.request(Command::NativeCatalog {
                    after: None,
                    limit: 1
                })?,
                WorkspaceReply::NativeCatalog { .. }
            ) {
                return Err("The Workspace host does not support native tasks".into());
            }
            let provider = AgentProvider::Codex;
            if description.title.trim().is_empty()
                || description.title.len() > 160
                || description.title.chars().any(char::is_control)
                || jterm_core::review_input::contains_visual_spoofing(&description.title)
            {
                return Err("Provide a visible task title of up to 160 bytes".into());
            }
            let verification = description
                .verification
                .as_ref()
                .map(|command| TaskValidationRecipe::new(command, VERIFICATION_SHELL))
                .transpose()?;
            let brief = TaskBrief::new(&description.goal, verification)?;
            if mode == Admission::Draft
                && std::iter::once(description.title.as_str())
                    .chain(std::iter::once(brief.goal.as_str()))
                    .chain(
                        brief
                            .validation
                            .as_ref()
                            .map(|recipe| recipe.command.as_str()),
                    )
                    .any(|text| jterm_core::redact::redact_secrets(text) != text)
            {
                return Err("Remove credentials from the task title, goal or verification recipe before saving".into());
            }
            let context = NativeLaunchContext::capture(provider)?;
            let worktrees =
                WorktreeService::new(native_worktrees_directory()?).map_err(|e| e.to_string())?;
            let name = format!("aether-{}", Uuid::new_v4());
            let branch = format!("aether/{name}");
            let created = worktrees
                .create(&CreateWorktreeRequest::new(
                    &description.repository,
                    &name,
                    &branch,
                    "HEAD",
                ))
                .map_err(|e| e.to_string())?;
            let mut manager = TaskManager::new();
            let id = manager
                .create_with_brief(
                    NewTask {
                        title: description.title.clone(),
                        provider,
                        repo_root: created.repository,
                        worktree_path: created.path,
                        branch: created.branch,
                        base_commit: created.head,
                        source_context: None,
                    },
                    brief,
                )
                .map_err(|e| e.to_string())?;
            let task = manager
                .get(id)
                .ok_or("Prepared task was unavailable")?
                .clone();
            self.prepared = Some(Prepared {
                description,
                mode,
                launch: NativeLaunch {
                    namespace: NAMESPACE.into(),
                    task,
                    context,
                    view: Uuid::new_v4().to_string(),
                    policy: NativePromptPolicy {
                        share_command_context: true,
                        redact_secrets: true,
                    },
                },
            });
        }
        let launch = self
            .prepared
            .as_ref()
            .expect("prepared native task")
            .launch
            .clone();
        let expected = task_id(&launch.namespace, &launch.task.id.to_string())?;
        if mode == Admission::Draft {
            let (snapshot, control) = native(self.request(Command::SaveNativeDraft {
                draft: Box::new(NativeDraft {
                    namespace: launch.namespace,
                    task: launch.task,
                    source_profile: launch.context.codex_home,
                }),
            })?)?;
            if snapshot.id != expected || control.is_some() {
                return Err("Workspace did not confirm the exact saved task draft".into());
            }
            return self.open(expected);
        }
        let token = launch.view.clone();
        let (snapshot, control) = native(self.request(Command::StartNative {
            launch: Box::new(launch),
        })?)?;
        if snapshot.id != expected {
            return Err("Workspace returned a different native task".into());
        }
        let result = self.install(snapshot, control, token)?;
        self.prepared = None;
        Ok(result)
    }

    fn open(&mut self, task: ConversationId) -> Result<Reply, String> {
        self.available()?;
        let original = self.inspect(&task)?;
        let token = Uuid::new_v4().to_string();
        let (snapshot, control) = self
            .request(Command::AttachNative {
                id: task.clone(),
                incarnation: original.incarnation.clone(),
                view: token.clone(),
            })
            .and_then(native)
            .unwrap_or((original.clone(), None));
        if snapshot.id != task || snapshot.incarnation != original.incarnation {
            return Err("Native task incarnation changed; reopen its current view".into());
        }
        let result = self.install(snapshot, control, token)?;
        if self.prepared.as_ref().is_some_and(|prepared| {
            task_id(NAMESPACE, &prepared.launch.task.id.to_string()).as_ref() == Ok(&task)
        }) {
            self.prepared = None;
        }
        Ok(result)
    }

    fn install(
        &mut self,
        snapshot: NativeSnapshot,
        control: Option<NativeControlLease>,
        token: String,
    ) -> Result<Reply, String> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or("Native task view exhausted")?;
        if control.as_ref().is_some_and(|lease| {
            lease.id != snapshot.id || lease.incarnation != snapshot.incarnation
        }) {
            return Err("Workspace returned a foreign native control lease".into());
        }
        let identity = Identity {
            task: snapshot.id.clone(),
            incarnation: snapshot.incarnation.clone(),
            generation,
        };
        self.active = Some(View {
            identity,
            token,
            sequence: snapshot.sequence,
            control,
            reviewed: None,
            context_preview: None,
        });
        self.generation = generation;
        self.project(snapshot)
    }

    fn decide(
        &mut self,
        identity: Identity,
        sequence: String,
        operation_id: String,
        decision: Decision,
    ) -> Result<Reply, String> {
        let view = self.selected(&identity)?;
        let sequence = number(&sequence)?;
        if sequence != view.sequence || Uuid::parse_str(&operation_id).is_err() {
            return Err("Native task evidence changed; review the current snapshot".into());
        }
        let control = view
            .control
            .clone()
            .ok_or("This view does not own native task control")?;
        let acceptance = match (&decision, &view.reviewed) {
            (Decision::AcceptVerified, Some(Review::Verified(reviewed)))
                if *reviewed == sequence =>
            {
                Some(NativeAction::MutateTask {
                    expected_sequence: sequence,
                    mutation: NativeTaskMutation::Complete,
                })
            }
            (
                Decision::AcceptManual,
                Some(Review::Manual {
                    sequence: reviewed,
                    fingerprint,
                }),
            ) if *reviewed == sequence => Some(NativeAction::AcceptReviewedAt {
                expected_sequence: sequence,
                fingerprint: fingerprint.clone(),
            }),
            (Decision::AcceptVerified | Decision::AcceptManual, _) => {
                return Err("Inspect the current worktree before accepting it".into());
            }
            _ => None,
        };
        let accepting = acceptance.is_some();
        let current = self.inspect(&identity.task)?;
        if current.incarnation != identity.incarnation {
            return Err("Native task incarnation changed".into());
        }
        let action = match decision {
            Decision::Decline { approval } => {
                if !current.live
                    || !current.view.as_ref().is_some_and(|view| {
                        view.pending_approvals
                            .iter()
                            .any(|pending| pending.id == approval)
                    })
                {
                    return Err("This approval request is no longer pending".into());
                }
                NativeAction::Approval {
                    id: approval,
                    decision: ApprovalDecision::Deny { reason: None },
                }
            }
            Decision::Stop if can_stop(&current) => NativeAction::Cancel,
            Decision::Finish if current.live && current.can_prompt => NativeAction::Finish,
            Decision::Verify if current.sequence == sequence && can_verify(&current) => {
                NativeAction::Validate {
                    expected_sequence: sequence,
                }
            }
            Decision::LaunchDraft {
                queue_revision,
                consent,
            } if current.sequence == sequence && current.saved_draft => {
                if !consent {
                    return Err("Confirm sharing before starting the saved task".into());
                }
                NativeAction::LaunchDraft {
                    expected_sequence: sequence,
                    expected_queue_revision: number(&queue_revision)?,
                    context: NativeLaunchContext::capture(current.task.provider)?,
                    policy: NativePromptPolicy {
                        share_command_context: true,
                        redact_secrets: true,
                    },
                }
            }
            Decision::AcceptVerified
                if current.sequence == sequence && can_accept_verified(&current) =>
            {
                acceptance.expect("verified review receipt")
            }
            Decision::AcceptManual
                if current.sequence == sequence && can_accept_manual(&current) =>
            {
                acceptance.expect("manual review receipt")
            }
            _ => return Err("This native task action is no longer available".into()),
        };
        if accepting {
            self.active.as_mut().expect("selected task").reviewed = None;
        }
        let (snapshot, _) = native(self.request(Command::ControlNative {
            control,
            operation: operation_id,
            action: Box::new(action),
        })?)?;
        if accepting && snapshot.task.status != TaskStatus::Completed {
            return Err("The Workspace host did not confirm task acceptance".into());
        }
        self.project(snapshot)
    }

    fn result(&mut self, identity: Identity) -> Result<Reply, String> {
        let view = self.selected(&identity)?;
        let sequence = view.sequence;
        let controlled = view.control.is_some();
        self.active.as_mut().expect("selected task").reviewed = None;
        let current = self.inspect(&identity.task)?;
        if current.incarnation != identity.incarnation || current.ownership_locked {
            return Err(
                "Finish the native session and verification before inspecting its worktree".into(),
            );
        }
        let WorkspaceReply::Result { report } = self.request(Command::InspectResult {
            id: identity.task.clone(),
        })?
        else {
            return Err("Missing native result inspection receipt".into());
        };
        if report.conversation != identity.task
            || report.run_id != identity.incarnation
            || report.task_id != current.task.id
        {
            return Err("Native result came from another execution".into());
        }
        if report.source_sequence != current.sequence || report.source_sequence != sequence {
            return Err("Task changed during inspection; inspect the current result again".into());
        }
        let can_accept = controlled && can_accept_verified(&current) && report.validation_current;
        let manual_review = report
            .current
            .as_ref()
            .filter(|_| controlled && can_accept_manual(&current))
            .map(|worktree| Review::Manual {
                sequence: report.source_sequence,
                fingerprint: worktree.fingerprint.clone(),
            });
        let can_accept_review = manual_review.is_some();
        self.active.as_mut().expect("selected task").reviewed = if can_accept {
            Some(Review::Verified(report.source_sequence))
        } else {
            manual_review
        };
        let (status, diff, truncated) = report.current.map_or_else(
            || (String::new(), String::new(), false),
            |current| {
                (
                    current.status,
                    current.diff,
                    current.diff_truncated || current.status_truncated,
                )
            },
        );
        Ok(Reply::Result {
            result: Box::new(ResultView {
                identity,
                sequence: report.source_sequence.to_string(),
                status,
                diff,
                truncated,
                validation: report.validation_status,
                can_accept,
                can_accept_review,
                detail: report.error,
            }),
        })
    }

    fn project(&mut self, mut snapshot: NativeSnapshot) -> Result<Reply, String> {
        let view = self.active.as_mut().ok_or("Select a native task")?;
        if view.identity.task != snapshot.id
            || view.identity.incarnation != snapshot.incarnation
            || snapshot.sequence < view.sequence
        {
            return Err("Stale or foreign native task projection".into());
        }
        if view.sequence != snapshot.sequence {
            view.reviewed = None;
            view.sequence = snapshot.sequence;
        }
        let controlled = view.control.is_some();
        let can_stop = controlled && can_stop(&snapshot);
        let can_verify = controlled && can_verify(&snapshot);
        let mut output = String::new();
        let mut approvals = Vec::new();
        let mut truncated = false;
        let mut detail = snapshot.task.status_detail.take();
        if let Some(provider) = snapshot.view {
            output = provider.agent_text;
            if snapshot.live {
                approvals = provider.pending_approvals;
            }
            truncated = provider.agent_text_truncated;
            detail = provider.last_error.or(detail);
        }
        if detail.is_none() {
            detail = snapshot.exit.and_then(|exit| exit.detail);
        }
        let (verification_output, verification_truncated) = snapshot.validation.map_or_else(
            || (String::new(), false),
            |validation| (validation.output, validation.output_truncated),
        );
        let verification = VerificationView {
            recipe: snapshot.task.validation_command().map(str::to_owned),
            shell: snapshot.task.validation_shell().map(str::to_owned),
            status: snapshot.task.validation.status.label(),
            output: verification_output,
            truncated: verification_truncated,
            can_start: can_verify,
        };
        let draft_goal = snapshot
            .task
            .brief
            .as_ref()
            .filter(|_| snapshot.saved_draft)
            .map(|brief| brief.goal.clone());
        Ok(Reply::Task {
            task: Box::new(TaskView {
                identity: view.identity.clone(),
                sequence: snapshot.sequence.to_string(),
                title: snapshot.task.title,
                status: snapshot.task.status.label(),
                worktree: snapshot.task.worktree_path,
                branch: snapshot.task.branch,
                detail,
                output,
                truncated,
                controlled,
                can_stop,
                can_finish: controlled && snapshot.live && snapshot.can_prompt,
                can_inspect: !snapshot.ownership_locked,
                draft_goal,
                approvals,
                verification,
            }),
        })
    }

    fn selected(&self, identity: &Identity) -> Result<&View, String> {
        self.active
            .as_ref()
            .filter(|view| &view.identity == identity)
            .ok_or_else(|| "Stale native task view".into())
    }

    fn inspect(&mut self, id: &ConversationId) -> Result<NativeSnapshot, String> {
        let (snapshot, _) = native(self.request(Command::InspectNative { id: id.clone() })?)?;
        if snapshot.id != *id {
            return Err("Workspace returned a different native task".into());
        }
        Ok(snapshot)
    }

    fn request(&mut self, command: Command) -> Result<WorkspaceReply, String> {
        super::request_host(&mut self.client, command)
    }
}

fn can_stop(snapshot: &NativeSnapshot) -> bool {
    snapshot.live
        || snapshot.preparing
        || (snapshot.ownership_locked
            && snapshot.task.validation.status == TaskValidationStatus::Running)
}

fn can_verify(snapshot: &NativeSnapshot) -> bool {
    !snapshot.ownership_locked
        && snapshot.can_start_validation
        && !snapshot.validation_evidence_redacted
        && snapshot.task.validation_command().is_some()
        && snapshot.task.validation_shell().is_some()
}

fn can_accept_verified(snapshot: &NativeSnapshot) -> bool {
    !snapshot.ownership_locked
        && !snapshot.validation_evidence_redacted
        && snapshot.task.status == TaskStatus::ReadyForReview
        && snapshot.task.validation.status == TaskValidationStatus::Passed
}

fn can_accept_manual(snapshot: &NativeSnapshot) -> bool {
    !snapshot.ownership_locked
        && snapshot.task.status == TaskStatus::ReadyForReview
        && snapshot.task.requires_manual_review()
        && snapshot.task.validation.status == TaskValidationStatus::NotRun
}

fn number(value: &str) -> Result<u64, String> {
    if value.len() > 20 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Invalid native task sequence or cursor".into());
    }
    value
        .parse()
        .map_err(|_| "Invalid native task sequence or cursor".into())
}

fn native(reply: WorkspaceReply) -> Result<(NativeSnapshot, Option<NativeControlLease>), String> {
    match reply {
        WorkspaceReply::Native { snapshot, control } => Ok((*snapshot, control)),
        _ => Err("Workspace did not return native task state".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_and_exact_view_precede_any_native_host_command() {
        let mut tasks = Tasks::default();
        let refused = tasks.execute(Action::Start {
            title: "Fixture".into(),
            repository: "/tmp/fixture".into(),
            goal: "Inspect fixture".into(),
            consent: false,
            verification: None,
        });
        assert!(matches!(refused, Err(error) if error.contains("Confirm sharing")));
        let id = Identity {
            task: ConversationId("jterm:task:fixture".into()),
            incarnation: Uuid::new_v4().to_string(),
            generation: 7,
        };
        tasks.active = Some(View {
            identity: id.clone(),
            token: "fixture".into(),
            sequence: 3,
            control: None,
            reviewed: None,
            context_preview: None,
        });
        let mut stale = id.clone();
        stale.generation = 6;
        assert!(tasks
            .execute(Action::Observe {
                identity: stale.clone()
            })
            .is_err());
        assert!(tasks
            .execute(Action::Context {
                identity: stale,
                choice: context::Choice::Read,
            })
            .is_err());
        let unreviewed = tasks.execute(Action::Context {
            identity: id.clone(),
            choice: context::Choice::Use {
                revision: "1".into(),
            },
        });
        assert!(matches!(unreviewed, Err(error) if error.contains("Preview the current context")));
        assert!(tasks
            .execute(Action::Decide {
                identity: id,
                sequence: "3".into(),
                operation_id: Uuid::new_v4().to_string(),
                decision: Decision::Decline {
                    approval: ApprovalId::new()
                },
            })
            .is_err());
        assert!(tasks.client.is_none() && tasks.prepared.is_none());
    }

    #[test]
    fn task_acceptance_requires_a_matching_result_receipt_from_this_view() {
        let identity = Identity {
            task: ConversationId("jterm:task:fixture".into()),
            incarnation: Uuid::new_v4().to_string(),
            generation: 7,
        };
        let mut tasks = Tasks {
            active: Some(View {
                identity: identity.clone(),
                token: "fixture".into(),
                sequence: 4,
                control: Some(NativeControlLease {
                    id: identity.task.clone(),
                    incarnation: identity.incarnation.clone(),
                    generation: Uuid::new_v4().to_string(),
                }),
                reviewed: Some(Review::Verified(3)),
                context_preview: None,
            }),
            ..Tasks::default()
        };
        let result = tasks.execute(Action::Decide {
            identity: identity.clone(),
            sequence: "4".into(),
            operation_id: Uuid::new_v4().to_string(),
            decision: Decision::AcceptVerified,
        });
        assert!(matches!(result, Err(error) if error.contains("Inspect the current")));
        tasks.active.as_mut().unwrap().reviewed = Some(Review::Verified(4));
        let manual = tasks.execute(Action::Decide {
            identity,
            sequence: "4".into(),
            operation_id: Uuid::new_v4().to_string(),
            decision: Decision::AcceptManual,
        });
        assert!(matches!(manual, Err(error) if error.contains("Inspect the current")));
        assert!(tasks.client.is_none());
    }
}
