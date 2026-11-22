use jterm_core::notebook::{CellRun, CellRunId, CellRunView, NotebookDocument, NotebookView};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::WebviewWindow;
use uuid::Uuid;

#[derive(Default)]
pub struct NotebookPanel {
    run: Option<CellRun>,
    document: Option<NotebookDocument>,
}

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Request {
    Inspect,
    Open {
        replacing: Option<Uuid>,
        name: String,
        source: String,
    },
    Run {
        document_id: Uuid,
        cell: usize,
    },
    Poll {
        document_id: Uuid,
        run_id: CellRunId,
        after: u64,
    },
    Stop {
        document_id: Uuid,
        run_id: CellRunId,
    },
    Close {
        document_id: Uuid,
    },
}

#[derive(Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum Response {
    State {
        document: Option<NotebookView>,
        run: Option<CellRunView>,
    },
    Document(NotebookView),
    Run(Option<CellRunView>),
    Stopping,
    Closed,
}

#[tauri::command]
pub async fn local_notebook<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    state: tauri::State<'_, Arc<Mutex<NotebookPanel>>>,
    request: Request,
) -> Result<Response, String> {
    super::require_terminal_window(&window)?;
    super::run_bridge(state.inner().clone(), |panel| panel.dispatch(request)).await
}

impl NotebookPanel {
    fn dispatch(&mut self, request: Request) -> Result<Response, String> {
        match request {
            Request::Inspect => Ok(Response::State {
                document: self
                    .document
                    .as_ref()
                    .map(|document| document.view().clone()),
                run: self.run.as_ref().map(CellRun::view),
            }),
            Request::Open {
                replacing,
                name,
                source,
            } => {
                if replacing
                    != self
                        .document
                        .as_ref()
                        .map(|document| document.view().document_id)
                {
                    return Err("The notebook selection has changed.".into());
                }
                self.require_idle()?;
                let document = NotebookDocument::open_local_snapshot(name, &source)
                    .map_err(|error| error.to_string())?;
                let view = document.view().clone();
                self.run = None;
                self.document = Some(document);
                Ok(Response::Document(view))
            }
            Request::Run { document_id, cell } => {
                self.require_idle()?;
                let run = self
                    .exact_document(document_id)?
                    .start(cell)
                    .map_err(|error| error.to_string())?;
                let view = run.view();
                self.run = Some(run);
                Ok(Response::Run(Some(view)))
            }
            Request::Poll {
                document_id,
                run_id,
                after,
            } => {
                let run = self.exact_run(document_id, run_id)?;
                Ok(Response::Run(run.view_after(after)))
            }
            Request::Stop {
                document_id,
                run_id,
            } => {
                self.exact_run(document_id, run_id)?.stop();
                Ok(Response::Stopping)
            }
            Request::Close { document_id } => {
                self.exact_document(document_id)?;
                self.require_idle()?;
                self.run = None;
                self.document = None;
                Ok(Response::Closed)
            }
        }
    }

    fn require_idle(&self) -> Result<(), String> {
        if self.run.as_ref().is_some_and(CellRun::is_running) {
            return Err("Stop the running notebook cell and wait for its receipt.".into());
        }
        Ok(())
    }

    fn exact_document(&self, id: Uuid) -> Result<&NotebookDocument, String> {
        self.document
            .as_ref()
            .filter(|document| document.view().document_id == id)
            .ok_or_else(|| "The notebook selection has changed.".into())
    }

    fn exact_run(&self, document_id: Uuid, run_id: CellRunId) -> Result<&CellRun, String> {
        self.exact_document(document_id)?;
        self.run
            .as_ref()
            .filter(|run| run.id() == run_id)
            .ok_or_else(|| "The notebook cell run has changed.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_notebook_cannot_run_close_or_replace_another_document() {
        let mut panel = NotebookPanel::default();
        let open = |replacing| Request::Open {
            replacing,
            name: "A.jtnb.md".into(),
            source: "```sh\nprintf ok\n```".into(),
        };
        let Response::Document(first) = panel.dispatch(open(None)).unwrap() else {
            panic!()
        };
        let Response::Document(current) = panel.dispatch(open(Some(first.document_id))).unwrap()
        else {
            panic!()
        };
        assert!(panel
            .dispatch(Request::Run {
                document_id: first.document_id,
                cell: 0
            })
            .is_err());
        assert!(panel
            .dispatch(Request::Close {
                document_id: first.document_id
            })
            .is_err());
        assert!(panel.dispatch(open(None)).is_err());
        assert!(panel
            .dispatch(Request::Close {
                document_id: current.document_id
            })
            .is_ok());
    }
}
