use jterm_core::workflows::{
    load_all, render, search_path, LoadOrder, PickerPolicy, SearchPathSpec, Workflow,
    WorkflowPicker, XdgEnvDirs,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tauri::WebviewWindow;
use tauri_terminal_frame_bridge::semantic::Attachment;
use uuid::Uuid;

use crate::local_sessions::{Counter, LocalSessions};

#[derive(Default)]
pub(crate) struct LocalWorkflows {
    catalog: Option<Catalog>,
}

struct Catalog {
    id: Uuid,
    owner: Attachment,
    prompt: Counter,
    picker: WorkflowPicker,
}

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum LibraryRequest {
    Open { owner: Attachment, prompt: Counter },
    Search { catalog_id: Uuid, query: String },
    Select { catalog_id: Uuid, name: String },
}

#[derive(Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum LibraryView {
    Results {
        catalog_id: Uuid,
        empty: bool,
        rows: Vec<WorkflowRow>,
    },
    Fields {
        name: String,
        command: String,
        args: Vec<ArgumentField>,
    },
}

#[derive(Serialize)]
pub(crate) struct WorkflowRow {
    name: String,
    description: String,
}

#[derive(Serialize)]
pub(crate) struct ArgumentField {
    name: String,
    description: String,
    default: Option<String>,
}

#[tauri::command]
pub(crate) async fn local_workflow_library<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    state: tauri::State<'_, Arc<Mutex<LocalWorkflows>>>,
    request: LibraryRequest,
) -> Result<LibraryView, String> {
    crate::require_terminal_window(&window)?;
    crate::run_bridge(state.inner().clone(), move |workflows| {
        workflows.library(request)
    })
    .await
}

#[tauri::command]
pub(crate) async fn insert_local_workflow<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    state: tauri::State<'_, Arc<Mutex<LocalWorkflows>>>,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
    owner: Attachment,
    catalog_id: Uuid,
    name: String,
    values: HashMap<String, String>,
) -> Result<Option<String>, String> {
    crate::require_terminal_window(&window)?;
    let sessions = bridge.inner().clone();
    crate::run_bridge(state.inner().clone(), move |workflows| {
        // Keep catalog validation and admission together. Scanning the library
        // and rendering its command never hold the live PTY registry lock.
        let result = workflows.insert(catalog_id, &owner, &name, &values, |prompt, command| {
            sessions
                .lock()
                .map_err(|_| "Terminal session unavailable.")?
                .input(&owner, |bridge, epoch| {
                    bridge.insert_prompt_workflow(epoch, prompt, command)
                })
        });
        Ok(result.err())
    })
    .await
}

impl LocalWorkflows {
    fn library(&mut self, request: LibraryRequest) -> Result<LibraryView, String> {
        match request {
            LibraryRequest::Open { owner, prompt } => {
                let paths = search_path(&SearchPathSpec::for_app("aether", None), &XdgEnvDirs);
                let catalog = Catalog {
                    id: Uuid::new_v4(),
                    owner,
                    prompt,
                    picker: WorkflowPicker::new(
                        load_all(&paths, LoadOrder::Precedence),
                        PickerPolicy::new(15, false),
                    ),
                };
                let view = catalog.results();
                self.catalog = Some(catalog);
                Ok(view)
            }
            LibraryRequest::Search { catalog_id, query } => {
                let catalog = self
                    .catalog
                    .as_mut()
                    .filter(|catalog| catalog.id == catalog_id)
                    .ok_or("Reopen Workflows to get the current library.")?;
                catalog.picker.set_query(query);
                Ok(catalog.results())
            }
            LibraryRequest::Select { catalog_id, name } => {
                let workflow = self.catalog(catalog_id)?.find(&name)?;
                Ok(LibraryView::Fields {
                    name,
                    command: workflow.command.clone(),
                    args: workflow
                        .args
                        .iter()
                        .map(|arg| ArgumentField {
                            name: arg.name.clone(),
                            description: arg.description.clone(),
                            default: arg.default.clone(),
                        })
                        .collect(),
                })
            }
        }
    }

    fn catalog(&self, id: Uuid) -> Result<&Catalog, String> {
        self.catalog
            .as_ref()
            .filter(|catalog| catalog.id == id)
            .ok_or_else(|| "Reopen Workflows to get the current library.".into())
    }

    fn insert(
        &mut self,
        id: Uuid,
        owner: &Attachment,
        name: &str,
        values: &HashMap<String, String>,
        enqueue: impl FnOnce(Counter, &str) -> Result<(), String>,
    ) -> Result<(), String> {
        let catalog = self.catalog(id)?;
        // A tab activation can rotate the view lease. The insertion still
        // targets the original PTY and prompt, while LocalSessions::input
        // independently validates the current writable attachment.
        if catalog.owner.session != owner.session || catalog.owner.epoch != owner.epoch {
            return Err("This workflow was selected for a different terminal session.".into());
        }
        let workflow = catalog.find(name)?;
        let command = render(workflow, values)?;
        enqueue(catalog.prompt, &command)?;
        self.catalog = None;
        Ok(())
    }
}

impl Catalog {
    fn find(&self, name: &str) -> Result<&Workflow, String> {
        self.picker
            .entries()
            .iter()
            .find(|workflow| workflow.name == name)
            .ok_or_else(|| "The selected workflow is unavailable.".into())
    }

    fn results(&self) -> LibraryView {
        LibraryView::Results {
            catalog_id: self.id,
            empty: self.picker.is_empty(),
            rows: self
                .picker
                .filtered()
                .into_iter()
                .map(|workflow| WorkflowRow {
                    name: workflow.name.clone(),
                    description: workflow.description.clone(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jterm_core::workflows::WorkflowArg;

    #[test]
    fn a_workflow_is_admitted_once_for_its_original_view_and_prompt() {
        let owner = Attachment {
            session: "fixture".into(),
            epoch: 1,
            generation: 2,
        };
        let prompt = Counter::new(7).unwrap();
        let id = Uuid::new_v4();
        let entry = Workflow {
            name: "Report".into(),
            description: String::new(),
            command: "printf '%s' '{value}'".into(),
            tags: Vec::new(),
            shell: None,
            args: vec![WorkflowArg {
                name: "value".into(),
                description: String::new(),
                default: None,
            }],
            source_path: None,
        };
        let mut workflows = LocalWorkflows {
            catalog: Some(Catalog {
                id,
                owner: owner.clone(),
                prompt,
                picker: WorkflowPicker::new(vec![entry], PickerPolicy::new(15, false)),
            }),
        };
        let refused = |_, _: &str| panic!("invalid request reached the PTY");
        let values = HashMap::from([("value".into(), "世界".into())]);
        let reattached = Attachment {
            generation: 3,
            ..owner.clone()
        };
        let foreign = Attachment {
            epoch: 2,
            ..reattached.clone()
        };
        assert!(workflows
            .insert(id, &foreign, "Report", &values, refused)
            .is_err());
        assert!(workflows
            .insert(id, &owner, "Report", &HashMap::new(), refused)
            .is_err());
        assert!(workflows
            .insert(id, &owner, "Report", &values, |_, _| Err(
                "The prompt changed.".into()
            ))
            .is_err());
        workflows
            .insert(
                id,
                &reattached,
                "Report",
                &values,
                |bound_prompt, command| {
                    assert_eq!(bound_prompt, prompt);
                    assert_eq!(command, "printf '%s' '世界'");
                    Ok(())
                },
            )
            .unwrap();
        assert!(workflows
            .insert(id, &owner, "Report", &values, refused)
            .is_err());
    }
}
