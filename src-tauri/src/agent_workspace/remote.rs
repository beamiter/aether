//! Read-only SSH Workspace UI. Commands are selected here, never forwarded from JS.
use super::tools::{Anchor, Document, Hit};
use jterm_core::agent_workspace::{
    controllers::remote::RemoteWorkspace, ConversationId, ConversationKind,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Default)]
pub(super) struct Viewer {
    reader: RemoteWorkspace,
    source: Option<Source>,
    revision: u32,
}

#[derive(Clone, Serialize)]
struct Source {
    view: Uuid,
    alias: String,
    host_id: String,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    Inspect,
    Connect {
        alias: String,
        executable: String,
        host_id: String,
    },
    Poll {
        view: Uuid,
    },
    Disconnect {
        view: Uuid,
    },
    Command {
        view: Uuid,
        revision: u32,
        command: Command,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Command {
    Refresh,
    NextCatalog,
    Search {
        query: String,
        conversation: Option<ConversationId>,
    },
    NextSearch,
    Read {
        anchor: Anchor,
    },
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Response {
    Closed,
    Unchanged,
    State { state: Box<View> },
}

#[derive(Serialize)]
pub(crate) struct View {
    source: Source,
    revision: u32,
    busy: bool,
    connected: bool,
    notice: Option<String>,
    rows: Vec<Row>,
    total: usize,
    more_rows: bool,
    query: String,
    conversation: Option<ConversationId>,
    hits: Vec<Hit>,
    more_hits: bool,
    coverage: Option<String>,
    document: Option<Document>,
}

#[derive(Serialize)]
struct Row {
    id: ConversationId,
    title: String,
    kind: ConversationKind,
    provider: String,
    directory: String,
    status: &'static str,
}

impl Viewer {
    pub(super) fn execute(&mut self, action: Action) -> Result<Response, String> {
        match action {
            Action::Inspect => {
                self.reader.poll();
            }
            Action::Connect {
                alias,
                executable,
                host_id,
            } => {
                if self.source.is_some() {
                    return Err("Disconnect the current remote Workspace first.".into());
                }
                self.reader.connect(&alias, &executable, &host_id);
                if !self.reader.loading {
                    return Err(self
                        .reader
                        .notice
                        .take()
                        .unwrap_or_else(|| "Could not start the remote Workspace query.".into()));
                }
                self.source = Some(Source {
                    view: Uuid::new_v4(),
                    alias: alias.trim().into(),
                    host_id: host_id.trim().into(),
                });
                self.revision = 0;
            }
            Action::Disconnect { view } => {
                self.require(view)?;
                self.reader.disconnect();
                self.source = None;
                return Ok(Response::Closed);
            }
            Action::Poll { view } => {
                self.require(view)?;
                if !self.reader.poll() {
                    return Ok(Response::Unchanged);
                }
            }
            Action::Command {
                view,
                revision,
                command,
            } => {
                self.require(view)?;
                if revision != self.revision || self.reader.busy() {
                    return Err("The remote Workspace selection changed. Refresh the view.".into());
                }
                self.command(command)?;
            }
        }
        let Some(source) = self.source.clone() else {
            return Ok(Response::Closed);
        };
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or("Remote Workspace view exhausted")?;
        let r = &self.reader;
        let document = match (&r.selected_hit, &r.document) {
            (Some(hit), Some(doc)) => Some(Document::exact(doc, Anchor::of(hit))?),
            _ => None,
        };
        Ok(Response::State {
            state: Box::new(View {
                source,
                revision: self.revision,
                busy: r.busy(),
                connected: r.connected,
                notice: r.notice.clone(),
                rows: r
                    .rows
                    .iter()
                    .map(|row| Row {
                        id: row.id.clone(),
                        title: row.title.clone(),
                        kind: row.kind,
                        provider: row.provider.clone(),
                        directory: row.cwd.clone(),
                        status: row.runtime.label(),
                    })
                    .collect(),
                total: r.total,
                more_rows: r.next_before.is_some(),
                query: r.query().into(),
                conversation: r.conversation().cloned(),
                hits: r.page.hits.iter().map(Hit::from).collect(),
                more_hits: r.page.next_before.is_some(),
                coverage: r.searching().then(|| r.page.coverage.label()),
                document,
            }),
        })
    }

    fn require(&self, view: Uuid) -> Result<(), String> {
        self.source
            .as_ref()
            .filter(|s| s.view == view)
            .map(|_| ())
            .ok_or_else(|| "Remote Workspace source changed.".into())
    }

    fn command(&mut self, command: Command) -> Result<(), String> {
        let r = &mut self.reader;
        if !r.connected && !matches!(command, Command::Refresh) {
            return Err("Connect to a verified remote Workspace first.".into());
        }
        match command {
            Command::Refresh | Command::NextCatalog => {
                r.set_search("", None);
                r.clear_document();
                if matches!(command, Command::Refresh) {
                    r.refresh();
                } else {
                    r.next_catalog_page(false);
                }
            }
            Command::Search {
                query,
                conversation,
            } => {
                let query = query.trim();
                if query.len() > 4096 || (query.is_empty() && conversation.is_none()) {
                    return Err("Enter a bounded query or select a remote conversation.".into());
                }
                if conversation
                    .as_ref()
                    .is_some_and(|id| !r.rows.iter().any(|row| &row.id == id))
                {
                    return Err("Choose a conversation from this remote catalog page.".into());
                }
                r.set_search("", None);
                r.set_search(query, conversation);
            }
            Command::NextSearch => {
                r.clear_document();
                r.next_search_page(false);
            }
            Command::Read { anchor } => {
                let hit = r
                    .page
                    .hits
                    .iter()
                    .find(|hit| Anchor::of(hit) == anchor)
                    .cloned()
                    .ok_or("Choose a record from this remote search page.")?;
                r.open_hit(hit);
            }
        }
        Ok(())
    }
}
