use jterm_core::{
    agent_sessions::{AgentSession, SessionCatalog},
    agent_workspace::{
        session_id, ConversationId, ConversationKind, RuntimeState, WorkspaceClient,
        WorkspaceCommand, WorkspaceEvent, WorkspaceQuery, WorkspaceReply, WorkspaceRow,
        WorkspaceSearchRequest, WORKSPACE_QUERY_MAX_BYTES, WORKSPACE_TEXT_BYTES,
    },
    review_input::{safe_inline_display, safe_multiline_display},
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    env,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    thread,
    time::{Duration, Instant},
};

const CATALOG_PAGE: usize = 120;
const HOST_PAGE: usize = 256;
const MAX_HOST_ROWS: usize = 2048;
const MAX_PROJECT_CHOICES: usize = 256;
const MAX_PROJECT_BYTES: usize = 2048;
const MAX_PROVIDER_BYTES: usize = 128;
const MAX_ID_BYTES: usize = 32768;
const HISTORY_PAGE: usize = 30;
const HOST_DEADLINE: Duration = Duration::from_secs(10);
const CLI_CONFIRMATION_LIFETIME: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub(crate) struct Workspace {
    sources: Arc<Sources>,
    catalog: Arc<Mutex<Option<Arc<Catalog>>>>,
    io_busy: Arc<AtomicBool>,
    catalog_busy: Arc<AtomicBool>,
    cli_deletion: Arc<Mutex<CliDeletion>>,
}

impl Workspace {
    pub(crate) fn from_paths(
        codex_home: Option<PathBuf>,
        claude_home: Option<PathBuf>,
        state_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            sources: Arc::new(Sources {
                codex_home,
                claude_home,
                state_dir,
            }),
            catalog: Arc::default(),
            io_busy: Arc::default(),
            catalog_busy: Arc::default(),
            cli_deletion: Arc::default(),
        }
    }

    fn loaded_catalog(&self) -> Result<Arc<Catalog>, String> {
        self.catalog
            .lock()
            .map_err(|_| "Workspace catalog unavailable")?
            .clone()
            .ok_or_else(|| "Load the conversation catalog first.".into())
    }
}

impl Default for Workspace {
    fn default() -> Self {
        let home = dirs::home_dir();
        let root = |variable, default| {
            env::var_os(variable)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .or_else(|| home.as_ref().map(|home| home.join(default)))
        };
        Self::from_paths(
            root("CODEX_HOME", ".codex"),
            root("CLAUDE_CONFIG_DIR", ".claude"),
            workspace_state_dir(),
        )
    }
}

struct Sources {
    codex_home: Option<PathBuf>,
    claude_home: Option<PathBuf>,
    state_dir: Option<PathBuf>,
}

struct Busy(Arc<AtomicBool>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

async fn background<T: Send + 'static>(
    busy: &Arc<AtomicBool>,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    busy.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .map_err(|_| "A workspace request is still finishing. Retry after it completes.")?;
    let active = Busy(Arc::clone(busy));
    tauri::async_runtime::spawn_blocking(move || {
        let _active = active;
        work()
    })
    .await
    .map_err(|_| "Workspace worker unavailable".to_owned())?
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    #[default]
    All,
    Cli,
    Api,
    Task,
}

impl Kind {
    fn qualifier(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Cli => Some("kind:cli"),
            Self::Api => Some("kind:api"),
            Self::Task => Some("kind:task"),
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Library {
    #[default]
    Active,
    Archived,
    All,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Activity {
    #[default]
    All,
    Running,
    NeedsYou,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Filter {
    query: String,
    kind: Kind,
    library: Library,
    pinned: bool,
    project: Option<String>,
    provider: Option<String>,
    activity: Activity,
}

struct Query {
    text: String,
    parsed: WorkspaceQuery,
    include_archived: bool,
}

fn admissible_project(value: &str) -> bool {
    Path::new(value).is_absolute()
        && value.len() <= MAX_PROJECT_BYTES
        && safe_inline_display(value, MAX_PROJECT_BYTES) == value
}

fn admissible_provider(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(',')
        && value.len() <= MAX_PROVIDER_BYTES
        && safe_inline_display(value, MAX_PROVIDER_BYTES) == value
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Filter {
    fn query(self) -> Result<Query, String> {
        // Reject malformed user input before appending UI-owned qualifiers.
        // In particular, an unterminated quote must not catch another term.
        WorkspaceQuery::parse(&self.query)?;
        let mut terms = vec![self.query];
        if let Some(kind) = self.kind.qualifier() {
            terms.push(kind.into());
        }
        if let Some(project) = self.project {
            if !admissible_project(&project) {
                return Err("Choose a valid absolute project path.".into());
            }
            terms.push(format!("project:{}", quote(&project)));
        }
        if let Some(provider) = self.provider {
            if !admissible_provider(&provider) {
                return Err("Choose a valid provider.".into());
            }
            terms.push(format!("provider:{}", quote(&provider)));
        }
        if self.pinned {
            terms.push("is:pinned".into());
        }
        match self.activity {
            Activity::All => {}
            Activity::Running => terms.push("is:running".into()),
            Activity::NeedsYou => terms.push("is:attention".into()),
        }
        if matches!(self.library, Library::Archived) {
            terms.push("is:archived".into());
        }
        let text = terms.join(" ");
        let parsed = WorkspaceQuery::parse(&text)?;
        let include_archived =
            !matches!(self.library, Library::Active) || parsed.includes_archived();
        Ok(Query {
            text,
            parsed,
            include_archived,
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Shared {
    Available,
    Unavailable,
    Partial,
}

struct Catalog {
    rows: Vec<WorkspaceRow>,
    cli_sources: HashMap<ConversationId, CliSource>,
    projects: Vec<String>,
    providers: Vec<String>,
    shared: Shared,
    truncated: bool,
    warnings: Vec<String>,
    cli_deletion_supported: bool,
}

#[derive(Clone)]
struct CliSource {
    session: AgentSession,
    indexed: bool,
}

#[derive(Default)]
struct CliDeletion {
    serial: u64,
    pending: Option<PendingCliDeletion>,
}

struct PendingCliDeletion {
    catalog: Weak<Catalog>,
    conversation: ConversationId,
    source: CliSource,
    token: String,
    expires: Instant,
}

#[derive(Serialize)]
pub(crate) struct CliDeleteConfirmation {
    token: String,
    title: String,
}

#[derive(Serialize)]
pub(crate) struct CliDeleteReceipt {
    conversation: String,
}

impl CliDeletion {
    fn prepare(
        &mut self,
        catalog: &Arc<Catalog>,
        conversation: ConversationId,
    ) -> Result<CliDeleteConfirmation, String> {
        let (source, title) = catalog.cli_deletion_target(&conversation)?;
        self.serial = self
            .serial
            .checked_add(1)
            .ok_or("Restart Aether to prepare another CLI deletion.")?;
        let token = self.serial.to_string();
        self.pending = Some(PendingCliDeletion {
            catalog: Arc::downgrade(catalog),
            conversation,
            source: source.clone(),
            token: token.clone(),
            expires: Instant::now() + CLI_CONFIRMATION_LIFETIME,
        });
        Ok(CliDeleteConfirmation {
            token,
            title: safe_inline_display(title, 512),
        })
    }

    fn take(&mut self, token: &str, current: &Arc<Catalog>) -> Result<PendingCliDeletion, String> {
        let stale = "The CLI deletion confirmation expired or changed. Open a new confirmation.";
        if self.pending.as_ref().is_none_or(|item| item.token != token) {
            return Err(stale.into());
        }
        let pending = self.pending.take().ok_or(stale)?;
        if pending.expires <= Instant::now() || !pending.catalog.ptr_eq(&Arc::downgrade(current)) {
            return Err(stale.into());
        }
        Ok(pending)
    }
}

#[derive(Serialize)]
pub(crate) struct CatalogPage {
    rows: Vec<Row>,
    total: usize,
    next_offset: Option<usize>,
    projects: Vec<String>,
    providers: Vec<String>,
    shared: Shared,
    truncated: bool,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct Row {
    id: String,
    kind: &'static str,
    title: String,
    provider: String,
    cwd: String,
    preview: String,
    updated_at_ms: u64,
    archived: bool,
    pinned: bool,
    tags: Vec<String>,
    runtime: String,
    running: bool,
    attention: Option<&'static str>,
    can_delete_cli: bool,
}

impl From<&WorkspaceRow> for Row {
    fn from(row: &WorkspaceRow) -> Self {
        let kind = match row.kind {
            ConversationKind::Api => "api",
            ConversationKind::Cli => "cli",
            ConversationKind::Task => "task",
        };
        let runtime = match (row.kind, row.runtime, row.archived) {
            (_, _, true) => "Archived".to_owned(),
            (ConversationKind::Cli, RuntimeState::Idle, false) => "Historical session".to_owned(),
            (_, RuntimeState::Running, false) => "Running".to_owned(),
            _ => safe_inline_display(&row.presentation().status_label, 128),
        };
        Self {
            id: row.id.0.clone(),
            kind,
            title: safe_inline_display(&row.title, 512),
            provider: safe_inline_display(&row.provider, MAX_PROVIDER_BYTES),
            cwd: safe_inline_display(&row.cwd, MAX_PROJECT_BYTES),
            preview: safe_multiline_display(&row.preview, 4096),
            updated_at_ms: row.updated_at_ms.min(8_640_000_000_000_000),
            archived: row.archived,
            pinned: row.pinned,
            tags: row
                .tags
                .iter()
                .take(16)
                .map(|tag| safe_inline_display(tag, 128))
                .collect(),
            runtime,
            running: row.runtime.is_live(),
            attention: row.attention().map(|attention| attention.label()),
            can_delete_cli: false,
        }
    }
}

impl Catalog {
    fn from_sources(
        local: SessionCatalog,
        shared_rows: Vec<WorkspaceRow>,
        shared: Shared,
        cli_deletion_supported: bool,
    ) -> Self {
        let mut rows: HashMap<_, _> = shared_rows
            .into_iter()
            .filter(|row| valid_identity(&row.id.0, MAX_ID_BYTES))
            .map(|row| (row.id.clone(), row))
            .collect();
        let mut cli_sources = HashMap::new();
        let live: HashSet<_> = local
            .live_processes
            .iter()
            .map(|process| (process.provider, process.id.as_str()))
            .collect();
        for session in local.sessions {
            let Ok(mut observed) = WorkspaceRow::from_agent(&session) else {
                continue;
            };
            if !valid_identity(&observed.id.0, MAX_ID_BYTES) {
                continue;
            }
            if live.contains(&(session.provider, session.id.as_str())) {
                observed.runtime = RuntimeState::Running;
            }
            cli_sources.insert(
                observed.id.clone(),
                CliSource {
                    session,
                    indexed: rows
                        .get(&observed.id)
                        .is_some_and(|row| row.kind == ConversationKind::Cli),
                },
            );
            match rows.entry(observed.id.clone()) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let indexed = entry.get_mut();
                    if indexed.kind == ConversationKind::Cli
                        && indexed.provider == observed.provider
                    {
                        indexed.archived |= observed.archived;
                        indexed.cwd = observed.cwd;
                        indexed.preview = observed.preview;
                        indexed.updated_at_ms = indexed.updated_at_ms.max(observed.updated_at_ms);
                        indexed.facts.branch = observed.facts.branch;
                        indexed.facts.recorded_status = observed.facts.recorded_status;
                        if indexed.runtime == RuntimeState::Idle {
                            indexed.runtime = observed.runtime;
                        }
                    }
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(observed);
                }
            }
        }
        let mut rows: Vec<_> = rows.into_values().collect();
        rows.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.updated_at_ms.cmp(&a.updated_at_ms))
                .then(a.id.cmp(&b.id))
        });
        let projects: BTreeSet<_> = rows
            .iter()
            .filter_map(|row| admissible_project(&row.cwd).then_some(row.cwd.as_str()))
            .collect();
        let truncated = shared == Shared::Partial
            || !local.warnings.is_empty()
            || projects.len() > MAX_PROJECT_CHOICES;
        let projects = projects
            .into_iter()
            .take(MAX_PROJECT_CHOICES)
            .map(str::to_owned)
            .collect();
        let providers = rows
            .iter()
            .filter_map(|row| admissible_provider(&row.provider).then_some(row.provider.as_str()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(32)
            .map(str::to_owned)
            .collect();
        Self {
            rows,
            cli_sources,
            projects,
            providers,
            shared,
            truncated,
            warnings: local.warnings,
            cli_deletion_supported,
        }
    }

    fn can_delete_cli(&self, row: &WorkspaceRow) -> bool {
        self.cli_deletion_supported
            && self.shared != Shared::Unavailable
            && row.kind == ConversationKind::Cli
            && !row.runtime.is_live()
            && self.cli_sources.contains_key(&row.id)
    }

    fn cli_deletion_target(&self, id: &ConversationId) -> Result<(&CliSource, &str), String> {
        let row = self
            .rows
            .iter()
            .find(|row| &row.id == id)
            .ok_or("The session is no longer in this catalog. Refresh before deleting it.")?;
        if row.kind == ConversationKind::Cli && row.runtime.is_live() {
            return Err("Stop the running CLI session before deleting it.".into());
        }
        if !self.can_delete_cli(row) {
            return Err(
                "Deletion requires a saved local CLI session and a current shared Workspace host."
                    .into(),
            );
        }
        let session = self
            .cli_sources
            .get(id)
            .ok_or("The original CLI session source is unavailable.")?;
        Ok((session, &row.title))
    }

    fn page(&self, query: Query, offset: usize) -> Result<CatalogPage, String> {
        let mut total = 0;
        let mut rows = Vec::new();
        for row in &self.rows {
            if (!query.include_archived && row.archived) || !query.parsed.matches(row) {
                continue;
            }
            if total >= offset && rows.len() < CATALOG_PAGE {
                let mut visible = Row::from(row);
                visible.can_delete_cli = self.can_delete_cli(row);
                rows.push(visible);
            }
            total += 1;
        }
        if offset > total {
            return Err(
                "The conversation page is out of date. Start again at the first page.".into(),
            );
        }
        let next = offset + rows.len();
        Ok(CatalogPage {
            rows,
            total,
            next_offset: (next < total).then_some(next),
            projects: self.projects.clone(),
            providers: self.providers.clone(),
            shared: self.shared,
            truncated: self.truncated,
            warnings: self.warnings.clone(),
        })
    }
}

fn workspace_state_dir() -> Option<PathBuf> {
    env::var_os("JTERM_WORKSPACE_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_STATE_HOME")
                .map(|value| PathBuf::from(value).join("jterm/agent-workspace"))
        })
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/state/jterm/agent-workspace")))
        .filter(|state| state.is_absolute())
}

// Explicit provider roots are also used by isolated instances. The pinned
// core's default discovery reads this table with the same read-only flags
// but its state directory is process-global. Check only IDs discovered here.
fn read_cli_deletions(
    path: &Path,
    identities: impl Iterator<Item = ConversationId>,
) -> Result<Option<HashSet<ConversationId>>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(_) => return Err("Workspace history is not a regular file.".into()),
        Err(error) => return Err(error.to_string()),
    }
    let mut db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|error| error.to_string())?;
    let transaction = db.transaction().map_err(|error| error.to_string())?;
    let has_table = transaction
        .prepare("SELECT 1 FROM sqlite_schema WHERE type='table' AND name='deleted_cli_sessions'")
        .and_then(|mut statement| statement.exists([]))
        .map_err(|error| error.to_string())?;
    if !has_table {
        return Ok(None);
    }
    let mut query = transaction
        .prepare("SELECT 1 FROM deleted_cli_sessions WHERE id=?1")
        .map_err(|error| error.to_string())?;
    let mut deleted = HashSet::new();
    for id in identities {
        if query.exists([&id.0]).map_err(|error| error.to_string())? {
            deleted.insert(id);
        }
    }
    Ok(Some(deleted))
}

impl Sources {
    fn local_catalog(&self) -> SessionCatalog {
        let unavailable = Path::new("/dev/null");
        let [codex, claude] = [self.codex_home.as_deref(), self.claude_home.as_deref()]
            .map(|path| path.filter(|path| path.is_absolute()));
        if codex.is_none() && claude.is_none() {
            return SessionCatalog {
                warnings: vec!["No local provider history directories are configured.".into()],
                ..Default::default()
            };
        }
        SessionCatalog::discover_with_roots(
            codex.unwrap_or(unavailable),
            claude.unwrap_or(unavailable),
        )
    }

    fn existing_host(&self) -> Result<WorkspaceClient, String> {
        let unavailable = "The local shared Agent Workspace host is unavailable.";
        let state = self
            .state_dir
            .as_ref()
            .filter(|path| path.is_absolute())
            .ok_or(unavailable)?;
        if !state.join("workspace.sqlite3").is_file() {
            return Err(unavailable.into());
        }
        // A conventional path has a socket beneath the state directory. The
        // shared client resolves long state paths to its private short socket path.
        if state.as_os_str().len() < 70
            && (!state.join("run/leases").is_dir()
                || !std::fs::symlink_metadata(state.join("run/host.sock"))
                    .is_ok_and(|metadata| metadata.file_type().is_socket()))
        {
            return Err(unavailable.into());
        }
        Ok(WorkspaceClient::for_state_dir(state))
    }

    fn catalog(&self) -> Catalog {
        let (mut rows, shared) = read_shared_catalog(self);
        let mut local = self.local_catalog();
        let mut cli_deletion_supported = false;
        if let Some(path) = self.state_dir.as_ref().filter(|path| path.is_absolute()) {
            let identities = local
                .sessions
                .iter()
                .filter_map(|session| session_id(session).ok())
                .chain(
                    rows.iter()
                        .filter(|row| row.kind == ConversationKind::Cli)
                        .map(|row| row.id.clone()),
                );
            match read_cli_deletions(&path.join("workspace.sqlite3"), identities) {
                Ok(Some(deleted)) => {
                    cli_deletion_supported = true;
                    rows.retain(|row| !deleted.contains(&row.id));
                    local.sessions.retain(|session| {
                        session_id(session).map_or(true, |id| !deleted.contains(&id))
                    });
                }
                Ok(None) => (),
                Err(_) => {
                    // A failed tombstone read cannot reintroduce CLI rows
                    // from the provider files the shared host deliberately kept.
                    local.sessions.clear();
                    local
                        .warnings
                        .push("Could not verify saved CLI deletions. Refresh to restore local CLI results.".into());
                }
            }
        }
        Catalog::from_sources(local, rows, shared, cli_deletion_supported)
    }
}

fn current_cli_source(
    catalog: SessionCatalog,
    pending: &PendingCliDeletion,
) -> Result<AgentSession, String> {
    let session = catalog
        .sessions
        .iter()
        .find(|session| {
            session.provider == pending.source.session.provider
                && session.id == pending.source.session.id
                && session.source == pending.source.session.source
                && session_id(session).is_ok_and(|id| id == pending.conversation)
        })
        .ok_or("The original CLI session source changed. Refresh before deleting it.")?;
    if !catalog.live_pids(session).is_empty() {
        return Err("Stop the running CLI session before deleting it.".into());
    }
    Ok(session.clone())
}

fn host_request(
    client: &WorkspaceClient,
    command: WorkspaceCommand,
    deadline: Instant,
) -> Result<WorkspaceReply, String> {
    let id = client.send(command)?;
    while Instant::now() < deadline {
        for WorkspaceEvent::Reply { request_id, result } in client.poll() {
            if request_id == id {
                return result.map_err(|error| safe_inline_display(&error, 512));
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err("The shared Agent Workspace host did not respond before the deadline.".into())
}

fn read_shared_catalog(sources: &Sources) -> (Vec<WorkspaceRow>, Shared) {
    let Ok(client) = sources.existing_host() else {
        return (Vec::new(), Shared::Unavailable);
    };
    let deadline = Instant::now() + HOST_DEADLINE;
    let mut catalog = Vec::new();
    let mut before = None;
    for _ in 0..MAX_HOST_ROWS / HOST_PAGE {
        let reply = host_request(
            &client,
            WorkspaceCommand::CatalogPage {
                before,
                limit: HOST_PAGE,
            },
            deadline,
        );
        let Ok(WorkspaceReply::CatalogPage {
            rows, next_before, ..
        }) = reply
        else {
            let shared = if catalog.is_empty() {
                Shared::Unavailable
            } else {
                Shared::Partial
            };
            return (catalog, shared);
        };
        catalog.extend(rows.into_iter().take(HOST_PAGE));
        if next_before.is_none() {
            return (catalog, Shared::Available);
        }
        if before.is_some_and(|cursor| next_before >= Some(cursor)) {
            break;
        }
        before = next_before;
    }
    (catalog, Shared::Partial)
}

#[derive(Serialize)]
struct Hit {
    row: Row,
    event_id: String,
    revision: String,
    source: &'static str,
    snippet: String,
    truncated: bool,
}

#[derive(Serialize)]
pub(crate) struct HistoryPage {
    hits: Vec<Hit>,
    next_before: Option<String>,
    coverage: String,
}

#[derive(Serialize)]
pub(crate) struct Document {
    source: &'static str,
    text: String,
    truncated: bool,
}

fn valid_identity(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn parse_cursor(before: Option<&str>) -> Result<Option<u64>, String> {
    before
        .map(|value| {
            value
                .parse::<i64>()
                .ok()
                .filter(|cursor| *cursor > 0 && cursor.to_string() == value)
                .map(|cursor| cursor as u64)
                .ok_or("Invalid history page cursor".to_owned())
        })
        .transpose()
}

#[tauri::command]
pub(crate) async fn workspace_refresh(
    filter: Filter,
    workspace: tauri::State<'_, Workspace>,
) -> Result<CatalogPage, String> {
    let query = filter.query()?;
    let cache = Arc::clone(&workspace.catalog);
    let sources = Arc::clone(&workspace.sources);
    background(&workspace.io_busy, move || {
        let catalog = Arc::new(sources.catalog());
        let page = catalog.page(query, 0)?;
        *cache.lock().map_err(|_| "Workspace catalog unavailable")? = Some(catalog);
        Ok(page)
    })
    .await
}

#[tauri::command]
pub(crate) async fn workspace_catalog(
    filter: Filter,
    offset: usize,
    workspace: tauri::State<'_, Workspace>,
) -> Result<CatalogPage, String> {
    if offset > MAX_HOST_ROWS + 2048 {
        return Err("Invalid conversation page offset".into());
    }
    let query = filter.query()?;
    let catalog = workspace.loaded_catalog()?;
    let cache = Arc::clone(&workspace.catalog);
    background(&workspace.catalog_busy, move || {
        let page = catalog.page(query, offset)?;
        if cache
            .lock()
            .map_err(|_| "Workspace catalog unavailable")?
            .as_ref()
            .is_none_or(|latest| !Arc::ptr_eq(latest, &catalog))
        {
            return Err("The conversation catalog changed. Refresh and try again.".into());
        }
        Ok(page)
    })
    .await
}

#[tauri::command]
pub(crate) fn workspace_prepare_cli_delete(
    conversation: String,
    workspace: tauri::State<'_, Workspace>,
) -> Result<CliDeleteConfirmation, String> {
    if !valid_identity(&conversation, MAX_ID_BYTES) {
        return Err("Invalid CLI conversation identity.".into());
    }
    let catalog = workspace.loaded_catalog()?;
    workspace
        .cli_deletion
        .lock()
        .map_err(|_| "CLI deletion confirmation unavailable")?
        .prepare(&catalog, ConversationId(conversation))
}

#[tauri::command]
pub(crate) async fn workspace_delete_cli_session(
    token: String,
    workspace: tauri::State<'_, Workspace>,
) -> Result<CliDeleteReceipt, String> {
    if token.is_empty() || token.len() > 20 || !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Invalid CLI deletion confirmation.".into());
    }
    let state = workspace.inner().clone();
    background(&workspace.io_busy, move || {
        let catalog = state.loaded_catalog()?;
        let pending = state
            .cli_deletion
            .lock()
            .map_err(|_| "CLI deletion confirmation unavailable")?
            .take(&token, &catalog)?;
        let client = state.sources.existing_host()?;
        let session = current_cli_source(state.sources.local_catalog(), &pending)?;
        let conversation = pending.conversation;
        let deadline = Instant::now() + HOST_DEADLINE;
        if !pending.source.indexed {
            // Aether may have discovered the source before another terminal
            // indexed it. The host only deletes indexed, source-qualified rows.
            let reply = host_request(
                &client,
                WorkspaceCommand::IndexSessions {
                    sessions: vec![WorkspaceRow::from_agent(&session)?],
                },
                deadline,
            )?;
            if !matches!(reply, WorkspaceReply::Ack) {
                return Err(
                    "The shared Workspace host did not index the selected CLI session.".into(),
                );
            }
        }
        let reply = host_request(
            &client,
            WorkspaceCommand::DeleteCliSession {
                id: conversation.clone(),
                session: Box::new(session),
            },
            deadline,
        )?;
        if !matches!(reply, WorkspaceReply::Ack) {
            return Err("The shared Workspace host did not confirm CLI deletion.".into());
        }
        *state
            .catalog
            .lock()
            .map_err(|_| "Workspace catalog unavailable")? = None;
        Ok(CliDeleteReceipt {
            conversation: conversation.0,
        })
    })
    .await
}

#[tauri::command]
pub(crate) async fn workspace_search(
    filter: Filter,
    before: Option<String>,
    workspace: tauri::State<'_, Workspace>,
) -> Result<HistoryPage, String> {
    if filter.query.trim().is_empty() || filter.query.len() > WORKSPACE_QUERY_MAX_BYTES {
        return Err("Enter a history search query within the workspace search limit.".into());
    }
    let query = filter.query()?;
    let before = parse_cursor(before.as_deref())?;
    let sources = Arc::clone(&workspace.sources);
    background(&workspace.io_busy, move || {
        let client = sources.existing_host()?;
        let reply = host_request(
            &client,
            WorkspaceCommand::Search {
                request: WorkspaceSearchRequest {
                    query: query.text,
                    conversation: None,
                    before,
                    limit: HISTORY_PAGE,
                    include_archived: query.include_archived,
                },
            },
            Instant::now() + HOST_DEADLINE,
        )?;
        let WorkspaceReply::Search { page } = reply else {
            return Err("Unexpected shared history response".into());
        };
        let hits = page
            .hits
            .into_iter()
            .take(HISTORY_PAGE)
            .filter(|hit| {
                valid_identity(&hit.row.id.0, MAX_ID_BYTES)
                    && valid_identity(&hit.anchor.event_id, 512)
                    && valid_identity(&hit.anchor.revision, 256)
            })
            .map(|hit| Hit {
                row: Row::from(&hit.row),
                event_id: hit.anchor.event_id,
                revision: hit.anchor.revision,
                source: hit.source.label(),
                snippet: safe_multiline_display(&hit.snippet, 4096),
                truncated: hit.truncated,
            })
            .collect();
        Ok(HistoryPage {
            hits,
            next_before: page.next_before.map(|cursor| cursor.to_string()),
            coverage: page.coverage.label(),
        })
    })
    .await
}

#[tauri::command]
pub(crate) async fn workspace_read_document(
    conversation: String,
    event_id: String,
    revision: String,
    workspace: tauri::State<'_, Workspace>,
) -> Result<Document, String> {
    if !valid_identity(&conversation, MAX_ID_BYTES)
        || !valid_identity(&event_id, 512)
        || !valid_identity(&revision, 256)
    {
        return Err("Invalid indexed history identity.".into());
    }
    let sources = Arc::clone(&workspace.sources);
    background(&workspace.io_busy, move || {
        let client = sources.existing_host()?;
        let conversation = ConversationId(conversation);
        let reply = host_request(
            &client,
            WorkspaceCommand::ReadSearchDocument {
                conversation: conversation.clone(),
                event_id: event_id.clone(),
                revision: revision.clone(),
            },
            Instant::now() + HOST_DEADLINE,
        )?;
        let WorkspaceReply::SearchDocument { document } = reply else {
            return Err("Unexpected indexed history response".into());
        };
        if document.conversation != conversation
            || document.event_id != event_id
            || document.revision != revision
        {
            return Err("Indexed history response did not match the selected revision.".into());
        }
        let text = safe_multiline_display(&document.text, WORKSPACE_TEXT_BYTES);
        Ok(Document {
            source: document.source.label(),
            truncated: document.truncated || text.len() < document.text.len(),
            text,
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use jterm_core::agent_sessions::{LiveSessionProcess, SessionProvider, SessionStatus};

    fn row(id: &str, kind: ConversationKind, cwd: &str, runtime: RuntimeState) -> WorkspaceRow {
        WorkspaceRow {
            id: ConversationId(id.into()),
            kind,
            title: "Release notes".into(),
            provider: "Codex".into(),
            cwd: cwd.into(),
            preview: String::new(),
            updated_at_ms: 1,
            archived: false,
            pinned: false,
            tags: Vec::new(),
            runtime,
            facts: Default::default(),
        }
    }

    #[test]
    fn same_structured_scope_applies_to_catalog_and_history() {
        let mut target = row(
            "target",
            ConversationKind::Task,
            "/work/\"app\"/src",
            RuntimeState::Running,
        );
        target.pinned = true;
        target.archived = true;
        let scope = Filter {
            query: "title:\"release notes\" -tag:draft".into(),
            kind: Kind::Task,
            library: Library::Archived,
            pinned: true,
            project: Some("/work/\"app\"".into()),
            provider: Some("Codex".into()),
            activity: Activity::Running,
        }
        .query()
        .unwrap();
        assert!(scope.include_archived);
        assert!(scope.parsed.matches(&target));
        let history = WorkspaceQuery::parse(&scope.text).unwrap();
        assert!(history.matches_document(&target, "entry"));
        for different in [
            WorkspaceRow {
                kind: ConversationKind::Api,
                ..target.clone()
            },
            WorkspaceRow {
                archived: false,
                ..target.clone()
            },
            WorkspaceRow {
                pinned: false,
                ..target.clone()
            },
            WorkspaceRow {
                cwd: "/work2/\"app\"/src".into(),
                ..target.clone()
            },
            WorkspaceRow {
                provider: "Claude".into(),
                ..target.clone()
            },
            WorkspaceRow {
                runtime: RuntimeState::Idle,
                ..target.clone()
            },
            WorkspaceRow {
                tags: vec!["draft".into()],
                ..target.clone()
            },
        ] {
            assert!(!scope.parsed.matches(&different));
            assert!(!history.matches_document(&different, "entry"));
        }
    }

    #[test]
    fn deduplicates_source_identity_and_preserves_shared_organization() {
        let observed = AgentSession {
            provider: SessionProvider::Codex,
            id: "f6bcd202-9952-4e12-bead-9e523267cea1".into(),
            title: "source title".into(),
            cwd: "/work/project".into(),
            updated_at_ms: 9,
            status: SessionStatus::Waiting,
            preview: "latest metadata".into(),
            branch: Some("main".into()),
            source: "/tmp/sessions/rollout-f6bcd202-9952-4e12-bead-9e523267cea1.jsonl".into(),
            archived: false,
        };
        let mut indexed = WorkspaceRow::from_agent(&observed).unwrap();
        indexed.title = "user alias".into();
        indexed.preview = "earlier indexed metadata".into();
        indexed.pinned = true;
        indexed.tags = vec!["demo".into()];
        indexed.updated_at_ms = 5;
        indexed.runtime = RuntimeState::Attached;
        let other = WorkspaceRow {
            id: ConversationId("another-home-same-native-id".into()),
            ..indexed.clone()
        };
        let catalog = Catalog::from_sources(
            SessionCatalog {
                live_processes: vec![LiveSessionProcess {
                    provider: observed.provider,
                    id: observed.id.clone(),
                    pid: 1,
                }],
                sessions: vec![observed],
                warnings: Vec::new(),
            },
            vec![indexed, other],
            Shared::Available,
            true,
        );
        let page = catalog
            .page(
                Filter {
                    query: "latest".into(),
                    ..Default::default()
                }
                .query()
                .unwrap(),
                0,
            )
            .unwrap();
        assert_eq!(catalog.rows.len(), 2);
        assert_eq!(page.total, 1);
        let actual = &page.rows[0];
        assert_eq!(actual.title, "user alias");
        assert!(actual.pinned && actual.running);
        assert_eq!(actual.tags, ["demo"]);
        assert_eq!(actual.updated_at_ms, 9);
        assert!(!actual.can_delete_cli);
        assert!(
            actual.attention.is_none(),
            "recorded input is not live approval"
        );
    }

    #[test]
    fn pagination_and_project_choices_keep_stable_identities() {
        let rows = (0..121)
            .map(|index| {
                let mut item = row(
                    &format!("id{index:03}"),
                    ConversationKind::Cli,
                    "/work/app",
                    RuntimeState::Idle,
                );
                item.updated_at_ms = index;
                item
            })
            .chain([row(
                "separate",
                ConversationKind::Api,
                "/tmp/app",
                RuntimeState::Idle,
            )])
            .collect();
        let catalog =
            Catalog::from_sources(SessionCatalog::default(), rows, Shared::Available, false);
        let page = catalog.page(Filter::default().query().unwrap(), 0).unwrap();
        assert_eq!(page.total, 122);
        assert_eq!(page.rows.len(), CATALOG_PAGE);
        assert_eq!(page.projects, ["/tmp/app", "/work/app"]);
        assert_eq!(page.next_offset, Some(120));
        let second = catalog
            .page(Filter::default().query().unwrap(), 120)
            .unwrap();
        assert_eq!(
            second
                .rows
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            ["separate", "id000"]
        );
        assert_eq!(second.next_offset, None);
    }

    #[test]
    fn query_and_wire_identities_have_fail_closed_bounds() {
        assert!(Filter {
            query: "title:\"incomplete".into(),
            kind: Kind::Cli,
            ..Default::default()
        }
        .query()
        .is_err());
        assert!(Filter {
            project: Some("../outside".into()),
            ..Default::default()
        }
        .query()
        .is_err());
        assert!(Filter {
            provider: Some("codex,claude".into()),
            ..Default::default()
        }
        .query()
        .is_err());
        assert_eq!(
            parse_cursor(Some("9007199254740997")).unwrap(),
            Some(9_007_199_254_740_997)
        );
        for invalid in ["0", "+2", "-5", "02", "9223372036854775808"] {
            assert!(parse_cursor(Some(invalid)).is_err());
        }
        assert!(!valid_identity("bad\0revision", 256));
        assert!(!valid_identity(&"a".repeat(257), 256));
    }

    fn saved_cli(home: &Path) -> AgentSession {
        let id = "01912345-1234-7123-8123-123456789abc";
        AgentSession {
            provider: SessionProvider::Codex,
            id: id.into(),
            title: "Owned workspace example".into(),
            cwd: home.to_path_buf(),
            updated_at_ms: 1,
            status: SessionStatus::Idle,
            preview: String::new(),
            branch: None,
            source: home
                .join("sessions")
                .join(format!("rollout-2026-11-20T01-02-03-{id}.jsonl")),
            archived: false,
        }
    }

    #[test]
    fn cli_confirmation_is_bound_to_the_selected_local_source_and_consumed_once() {
        let session = saved_cli(Path::new("/tmp"));
        let id = session_id(&session).unwrap();
        let catalog = || {
            Arc::new(Catalog::from_sources(
                SessionCatalog {
                    sessions: vec![session.clone()],
                    ..Default::default()
                },
                vec![row(
                    "api",
                    ConversationKind::Api,
                    "/tmp",
                    RuntimeState::Idle,
                )],
                Shared::Available,
                true,
            ))
        };
        let current = catalog();
        let page = current.page(Filter::default().query().unwrap(), 0).unwrap();
        assert_eq!(page.rows.iter().filter(|row| row.can_delete_cli).count(), 1);
        assert!(!Row::from(&WorkspaceRow::from_agent(&session).unwrap()).can_delete_cli);

        let mut confirmation = CliDeletion::default();
        assert!(confirmation
            .prepare(&current, ConversationId("api".into()))
            .is_err());
        let discarded = confirmation.prepare(&current, id.clone()).unwrap();
        let accepted = confirmation.prepare(&current, id.clone()).unwrap();
        assert_eq!(accepted.title, session.title);
        assert!(confirmation.take(&discarded.token, &current).is_err());
        let pending = confirmation.take(&accepted.token, &current).unwrap();
        assert!(confirmation.take(&accepted.token, &current).is_err());
        let superseded = confirmation.prepare(&current, id.clone()).unwrap();
        assert!(confirmation.take(&superseded.token, &catalog()).is_err());
        let expired = confirmation.prepare(&current, id).unwrap();
        confirmation.pending.as_mut().unwrap().expires = Instant::now();
        assert!(confirmation.take(&expired.token, &current).is_err());

        let same_source = SessionCatalog {
            sessions: vec![session.clone()],
            ..Default::default()
        };
        assert_eq!(current_cli_source(same_source, &pending).unwrap(), session);
        let changed = SessionCatalog {
            sessions: vec![AgentSession {
                source: session.source.with_file_name("unexpected.jsonl"),
                ..session.clone()
            }],
            ..Default::default()
        };
        assert!(current_cli_source(changed, &pending).is_err());
        let running = SessionCatalog {
            live_processes: vec![LiveSessionProcess {
                provider: session.provider,
                id: session.id.clone(),
                pid: 1,
            }],
            sessions: vec![session],
            warnings: Vec::new(),
        };
        assert!(current_cli_source(running, &pending).is_err());
    }

    #[test]
    fn durable_cli_deletion_filters_only_the_exact_profile_during_offline_discovery() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("aether-delete-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let fixture = Fixture(path);
        let one = saved_cli(&fixture.0.join("one"));
        let two = saved_cli(&fixture.0.join("two"));
        for session in [&one, &two] {
            std::fs::create_dir_all(session.source.parent().unwrap()).unwrap();
            let metadata = serde_json::json!({"type": "session_meta", "payload": {
                "id": session.id, "cwd": session.cwd,
            }});
            std::fs::write(&session.source, format!("{metadata}\n")).unwrap();
        }
        let discover = |session: &AgentSession| {
            Sources {
                codex_home: Some(session.cwd.clone()),
                claude_home: None,
                state_dir: Some(fixture.0.clone()),
            }
            .catalog()
        };
        let initial = discover(&one);
        assert_eq!(initial.rows.len(), 1);
        assert!(!initial.can_delete_cli(&initial.rows[0]));

        let db = Connection::open(fixture.0.join("workspace.sqlite3")).unwrap();
        db.execute(
            "CREATE TABLE deleted_cli_sessions (id TEXT PRIMARY KEY)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO deleted_cli_sessions (id) VALUES (?1)",
            [&session_id(&one).unwrap().0],
        )
        .unwrap();
        drop(db);
        let original = std::fs::read(&one.source).unwrap();
        assert!(discover(&one).rows.is_empty());
        assert_eq!(discover(&two).rows[0].id, session_id(&two).unwrap());
        assert_eq!(std::fs::read(&one.source).unwrap(), original);
    }
}
