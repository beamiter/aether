//! Aether views over the shared, read-only Mission workbench.
use super::native_tasks;
use jterm_core::agent_workspace::{
    MissionWorkbench, NativeSnapshot, WorkspaceMission, WorkspaceMissionReport,
    WorkspaceMissionRepository,
};
use std::{
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    id: String,
    revision: String,
    generation: u32,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, tag = "operation", rename_all = "snake_case")]
pub(crate) enum Choice {
    List,
    Open {
        id: String,
    },
    Create {
        source: native_tasks::Identity,
        title: String,
        goal: String,
        repository: String,
        target_ref: String,
    },
    Save {
        identity: Identity,
        title: String,
        goal: String,
    },
    Inspect {
        identity: Identity,
    },
    Freeze {
        identity: Identity,
    },
}

impl Choice {
    pub(super) fn source(&self) -> Option<&native_tasks::Identity> {
        match self {
            Self::Create { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(serde::Serialize)]
pub(crate) struct Reply {
    rows: Vec<Row>,
    view: Option<View>,
}

#[derive(serde::Serialize)]
struct Row {
    id: String,
    title: String,
}

#[derive(serde::Serialize)]
struct View {
    identity: Identity,
    title: String,
    goal: String,
    environment: String,
    repositories: Vec<WorkspaceMissionRepository>,
    report: Option<WorkspaceMissionReport>,
    current_inspection: bool,
    can_freeze: bool,
}

#[derive(Default)]
pub(super) struct Missions {
    workbench: MissionWorkbench,
    generation: u32,
}

impl Missions {
    pub(super) fn execute(
        &mut self,
        choice: Choice,
        source: Option<NativeSnapshot>,
    ) -> Result<Reply, String> {
        self.settle()?;
        self.workbench.notice = None;
        match choice {
            Choice::List => {
                self.workbench.refresh();
                self.confirm()?;
                return Ok(self.reply(None));
            }
            Choice::Open { id } => {
                if id.is_empty() || id.len() > 128 {
                    return Err("Choose a saved Mission".into());
                }
                self.workbench.select(&id);
                self.confirm()?;
                if self.selected()?.id != id {
                    return Err("Unexpected Mission selection receipt".into());
                }
                self.next_generation()?;
            }
            Choice::Create {
                title,
                goal,
                repository,
                target_ref,
                ..
            } => {
                let source = source.ok_or("Select the native task to link to this Mission")?;
                self.workbench.create_for_native_task(
                    &title,
                    &goal,
                    &repository,
                    &target_ref,
                    &source,
                );
                self.confirm()?;
                self.next_generation()?;
            }
            Choice::Save {
                identity,
                title,
                goal,
            } => {
                let mut mission = self.current(&identity)?.clone();
                mission.title = title;
                mission.goal = goal;
                self.workbench.save(mission);
                self.confirm()?;
                self.confirm_write(&identity)?;
            }
            Choice::Inspect { identity } => {
                self.current(&identity)?;
                self.workbench.inspect();
                self.confirm()?;
                let mission = self.current(&identity)?;
                if !self.workbench.report.as_ref().is_some_and(|report| {
                    report.id == mission.id && report.revision == mission.revision
                }) {
                    return Err("Missing inspection for the current Mission revision".into());
                }
                return Ok(self.reply(Some(self.project(mission, true))));
            }
            Choice::Freeze { identity } => {
                self.current(&identity)?;
                if !self.workbench.can_adopt_candidates() {
                    return Err(
                        "Inspect clean repository evidence before freezing candidates".into(),
                    );
                }
                self.workbench.adopt_candidates();
                self.confirm()?;
                self.confirm_write(&identity)?;
            }
        }
        Ok(self.reply(Some(self.project(self.selected()?, false))))
    }

    fn settle(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            self.workbench.poll();
            if !self.workbench.busy {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(
                    "The Mission response is still pending. Reload before retrying.".into(),
                );
            }
            thread::sleep(Duration::from_millis(15));
        }
    }

    fn confirm(&mut self) -> Result<(), String> {
        self.settle()?;
        self.workbench.notice.take().map_or(Ok(()), Err)
    }

    fn next_generation(&mut self) -> Result<(), String> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("Mission selection exhausted")?;
        Ok(())
    }

    fn selected(&self) -> Result<&WorkspaceMission, String> {
        self.workbench
            .selected
            .as_ref()
            .filter(|mission| mission.revision > 0)
            .ok_or_else(|| "Select a saved Mission".into())
    }

    fn identity(&self, mission: &WorkspaceMission) -> Identity {
        Identity {
            id: mission.id.clone(),
            revision: mission.revision.to_string(),
            generation: self.generation,
        }
    }

    fn current(&self, identity: &Identity) -> Result<&WorkspaceMission, String> {
        let mission = self.selected()?;
        if self.identity(mission) != *identity {
            return Err("This Mission revision changed. Open its saved plan again.".into());
        }
        Ok(mission)
    }

    fn confirm_write(&self, previous: &Identity) -> Result<(), String> {
        let mission = self.selected()?;
        if mission.id != previous.id
            || mission
                .revision
                .checked_sub(1)
                .map(|revision| revision.to_string())
                .as_ref()
                != Some(&previous.revision)
        {
            return Err("The Mission changed during the write. Reload the saved plan.".into());
        }
        Ok(())
    }

    fn reply(&self, view: Option<View>) -> Reply {
        Reply {
            rows: self
                .workbench
                .missions
                .iter()
                .map(|mission| Row {
                    id: mission.id.clone(),
                    title: mission.title.clone(),
                })
                .collect(),
            view,
        }
    }

    fn project(&self, mission: &WorkspaceMission, current_inspection: bool) -> View {
        let mut report = self
            .workbench
            .report
            .clone()
            .filter(|report| report.id == mission.id && report.revision == mission.revision);
        if !current_inspection {
            if let Some(report) = &mut report {
                report.require_reinspection();
            }
        }
        View {
            identity: self.identity(mission),
            title: mission.title.clone(),
            goal: mission.goal.clone(),
            environment: mission.required_environment.clone(),
            repositories: mission.repositories.clone(),
            report,
            current_inspection,
            can_freeze: current_inspection && self.workbench.can_adopt_candidates(),
        }
    }
}
