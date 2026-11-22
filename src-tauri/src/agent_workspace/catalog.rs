use jterm_core::{
    agent_sessions::{AgentSession, SessionCatalog, SessionLibrarySnapshot},
    agent_workspace::{
        ConversationId, ConversationKind, WorkspaceCliCatalogIndex, WorkspaceClient,
        WorkspaceCommand, WorkspaceReply,
    },
};
use std::collections::{HashMap, HashSet};

const PAGE_ROWS: usize = 32;

#[derive(Default)]
pub(super) struct Catalog {
    client: Option<WorkspaceClient>,
    sources: HashMap<ConversationId, AgentSession>,
    shown: HashSet<ConversationId>,
    generation: u32,
    next: Option<u64>,
}

#[derive(serde::Serialize)]
pub(crate) struct Page {
    generation: u32,
    rows: Vec<Row>,
    next_before: Option<String>,
}

#[derive(serde::Serialize)]
struct Row {
    id: ConversationId,
    title: String,
    provider: String,
    directory: String,
    status: &'static str,
    reopenable: bool,
}

impl Catalog {
    pub(super) fn page(&mut self, cursor: Option<(u32, String)>) -> Result<Page, String> {
        let before = if let Some((generation, before)) = cursor {
            let before = before
                .parse::<u64>()
                .map_err(|_| "Invalid Workspace page cursor")?;
            if generation != self.generation || self.next != Some(before) {
                return Err("Workspace page changed; refresh the catalog".into());
            }
            Some(before)
        } else {
            let generation = self
                .generation
                .checked_add(1)
                .ok_or("Workspace catalog exhausted")?;
            let catalog = SessionCatalog::discover();
            let (index, rows) =
                WorkspaceCliCatalogIndex::prepare(&catalog, &SessionLibrarySnapshot::default());
            let admitted = rows.iter().map(|row| &row.id).collect::<HashSet<_>>();
            let sources = catalog
                .sessions
                .iter()
                .filter_map(|session| {
                    let id = index.identity(session)?;
                    admitted.contains(id).then(|| (id.clone(), session.clone()))
                })
                .collect();
            if !rows.is_empty() {
                match self.request(WorkspaceCommand::IndexSessions { sessions: rows })? {
                    WorkspaceReply::Ack => {}
                    _ => return Err("Unexpected Workspace index receipt".into()),
                }
            }
            self.sources = sources;
            self.shown.clear();
            self.generation = generation;
            self.next = None;
            None
        };
        let WorkspaceReply::CatalogPage {
            rows, next_before, ..
        } = self.request(WorkspaceCommand::CatalogPage {
            before,
            limit: PAGE_ROWS,
        })?
        else {
            return Err("Unexpected Workspace catalog receipt".into());
        };
        let rows = rows
            .into_iter()
            .filter(|row| row.kind == ConversationKind::Cli)
            .map(|row| {
                let reopenable = !row.archived && self.sources.contains_key(&row.id);
                if reopenable {
                    self.shown.insert(row.id.clone());
                }
                Row {
                    reopenable,
                    id: row.id,
                    title: row.title,
                    provider: row.provider,
                    directory: row.cwd,
                    status: row.runtime.label(),
                }
            })
            .collect();
        self.next = next_before;
        Ok(Page {
            generation: self.generation,
            rows,
            next_before: next_before.map(|before| before.to_string()),
        })
    }

    pub(super) fn source(
        &self,
        generation: u32,
        id: &ConversationId,
    ) -> Result<AgentSession, String> {
        if generation != self.generation || !self.shown.contains(id) {
            return Err("Workspace selection changed; refresh and select the conversation".into());
        }
        self.sources
            .get(id)
            .cloned()
            .ok_or_else(|| "Workspace conversation no longer resolves to a local CLI source".into())
    }

    fn request(&mut self, command: WorkspaceCommand) -> Result<WorkspaceReply, String> {
        super::request_host(&mut self.client, command)
    }
}
