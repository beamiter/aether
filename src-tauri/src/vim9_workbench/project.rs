//! Project tools use the shared immutable file index and Vim's authenticated metadata.
use jterm_core::vim9::{
    vim9_palette_matches, vim9_palette_query, Vim9Build, Vim9Command, Vim9Diagnostic,
    Vim9PaletteCommand, Vim9ProjectIndex, Vim9ProjectSnapshot, Vim9Quickfix, Vim9Search,
    Vim9Session, Vim9Snapshot,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, mem, path::Path, sync::Arc};
use uuid::Uuid;

const MENU_LIMIT: usize = 64;

#[derive(Default)]
pub(super) struct Tools {
    index: Option<Vim9ProjectIndex>,
    menu: Option<Menu>,
    activity: Option<ActivityUpdate>,
}

#[derive(PartialEq, Eq)]
struct Cursor {
    window: i64,
    buffer: i64,
    changedtick: u64,
    line: u64,
    column: u64,
}
impl From<&Vim9Snapshot> for Cursor {
    fn from(s: &Vim9Snapshot) -> Self {
        Self {
            window: s.window,
            buffer: s.buffer,
            changedtick: s.changedtick,
            line: s.line,
            column: s.column,
        }
    }
}

struct Menu {
    id: Uuid,
    source: Cursor,
    actions: BTreeMap<String, Target>,
}

enum Target {
    Palette(Vim9PaletteCommand),
    Navigation(Vim9Command),
}

#[derive(Serialize)]
pub(crate) struct Choices {
    id: Uuid,
    scanning: bool,
    truncated: bool,
    entries: Vec<Choice>,
}

#[derive(Serialize)]
struct Choice {
    key: String,
    title: String,
    directory: bool,
    unavailable: Option<&'static str>,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub(super) struct Activity {
    root: String,
    build: Vim9Build,
    search: Vim9Search,
    quickfix: Vim9Quickfix,
    diagnostics: Vec<Vim9Diagnostic>,
    diagnostics_truncated: bool,
}

#[derive(Clone, Serialize)]
pub(crate) struct ActivityUpdate {
    revision: u32,
    value: Arc<Activity>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Job {
    Search { query: String },
    Build { command: String },
    CancelBuild,
    CancelSearch,
    NextProblem,
    PreviousProblem,
}

impl Job {
    pub(super) fn command(self, state: &Vim9Snapshot) -> Result<Vim9Command, String> {
        let palette = match self {
            Self::Search { query } => {
                valid_input(&query)?;
                if state.search.running {
                    return Err("A project search is already running.".into());
                }
                return Ok(Vim9Command::Search { query });
            }
            Self::Build { command } => {
                valid_input(&command)?;
                if state.build.running {
                    return Err("A Vim build is already running.".into());
                }
                return Ok(Vim9Command::Build { command });
            }
            Self::CancelBuild => Vim9PaletteCommand::CancelBuild,
            Self::CancelSearch => Vim9PaletteCommand::CancelSearch,
            Self::NextProblem => Vim9PaletteCommand::NextDiagnostic,
            Self::PreviousProblem => Vim9PaletteCommand::PreviousDiagnostic,
        };
        palette.command(state).map_err(String::from)
    }
}

fn valid_input(value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 4096 || value.contains(['\0', '\r', '\n']) {
        Err("Enter a non-empty single line up to 4096 bytes.".into())
    } else {
        Ok(())
    }
}

impl Tools {
    pub(super) fn lookup(
        &mut self,
        vim: &Vim9Session,
        query: &str,
        folder: &str,
        refresh: bool,
    ) -> Result<Choices, String> {
        self.menu = None;
        if query.chars().count() > 256 || query.contains(['\r', '\n', '\0']) || folder.len() > 4096
        {
            return Err("Invalid project query.".into());
        }
        let state = vim.snapshot();
        if !state.connected {
            return Err("Vim's project bridge is disconnected.".into());
        }
        let mut menu = Menu {
            id: Uuid::new_v4(),
            source: Cursor::from(&state),
            actions: BTreeMap::new(),
        };
        let mut result = Choices {
            id: menu.id,
            scanning: false,
            truncated: false,
            entries: Vec::new(),
        };
        if let Some(filter) = vim9_palette_query(query) {
            for command in vim9_palette_matches(filter, MENU_LIMIT) {
                let title = command.title().to_owned();
                result.entries.push(Choice {
                    key: title.clone(),
                    title: title.clone(),
                    directory: false,
                    unavailable: command.unavailable_reason(&state),
                });
                menu.actions.insert(title, Target::Palette(command));
            }
        } else {
            if refresh {
                self.index = None;
            }
            let index = self
                .index
                .get_or_insert_with(|| Vim9ProjectIndex::start(vim.root()))
                .snapshot();
            if let Some(error) = &index.error {
                return Err(error.clone());
            }
            result.scanning = index.scanning;
            result.truncated = index.truncated;
            if query.trim().is_empty() {
                menu.directory(&mut result, &index, folder);
            } else {
                let parsed = index.quick_open_query(query)?;
                if parsed.current_buffer {
                    menu.navigation(&mut result, query, parsed.command(None)?);
                } else {
                    let files = index.matches(&parsed.file_query, MENU_LIMIT + 1);
                    result.truncated |= files.len() > MENU_LIMIT;
                    for file in files.iter().take(MENU_LIMIT) {
                        menu.navigation(&mut result, &file.relative, parsed.command(Some(file))?);
                    }
                }
            }
        }
        self.menu = Some(menu);
        Ok(result)
    }

    pub(super) fn choose(
        &mut self,
        vim: &Vim9Session,
        id: Uuid,
        key: &str,
    ) -> Result<Vim9Command, String> {
        let menu = self
            .menu
            .as_mut()
            .filter(|menu| menu.id == id)
            .ok_or("The project choices changed. Find again.")?;
        let state = vim.snapshot();
        if !state.connected || menu.source != Cursor::from(&state) {
            return Err("Vim's source position changed. Find again.".into());
        }
        let target = menu
            .actions
            .remove(key)
            .ok_or("Choose an available result from the current project list.")?;
        self.menu = None;
        match target {
            Target::Palette(command) => command.command(&state).map_err(String::from),
            Target::Navigation(command) => {
                if let Vim9Command::Open { path, .. } = &command {
                    let resolved = Path::new(path).canonicalize().map_err(|e| e.to_string())?;
                    if !resolved.starts_with(vim.root()) || !resolved.is_file() {
                        return Err(
                            "The selected file no longer belongs to the open Vim project.".into(),
                        );
                    }
                }
                Ok(command)
            }
        }
    }

    pub(super) fn activity(
        &mut self,
        root: &Path,
        state: &mut Vim9Snapshot,
        seen: u32,
    ) -> Result<Option<ActivityUpdate>, String> {
        let value = Activity {
            root: root.to_string_lossy().into_owned(),
            build: mem::take(&mut state.build),
            search: mem::take(&mut state.search),
            quickfix: mem::take(&mut state.quickfix),
            diagnostics: mem::take(&mut state.diagnostics),
            diagnostics_truncated: state.diagnostics_truncated,
        };
        if self.activity.as_ref().is_none_or(|a| *a.value != value) {
            let revision = self
                .activity
                .as_ref()
                .map_or(0, |a| a.revision)
                .checked_add(1)
                .ok_or("Vim project revision exhausted")?;
            self.activity = Some(ActivityUpdate {
                revision,
                value: Arc::new(value),
            });
        }
        Ok(self
            .activity
            .as_ref()
            .filter(|a| a.revision != seen)
            .cloned())
    }
}

impl Menu {
    fn navigation(&mut self, result: &mut Choices, title: &str, command: Vim9Command) {
        self.actions
            .insert(title.into(), Target::Navigation(command));
        result.entries.push(Choice {
            key: title.into(),
            title: title.into(),
            directory: false,
            unavailable: None,
        });
    }

    fn directory(&mut self, result: &mut Choices, index: &Vim9ProjectSnapshot, folder: &str) {
        let prefix = if folder.is_empty() {
            String::new()
        } else {
            format!("{folder}/")
        };
        // The bounded index is rooted in the file chosen by the user. Folder
        // names from the WebView only filter it and never initiate filesystem reads.
        let mut children = BTreeMap::new();
        for file in index.files.iter() {
            let Some(tail) = file.relative.strip_prefix(&prefix) else {
                continue;
            };
            match tail.split_once('/') {
                Some((name, _)) => {
                    children.insert((false, name), None);
                }
                None => {
                    children.insert((true, tail), Some(file));
                }
            }
        }
        result.truncated |= children.len() > MENU_LIMIT;
        for ((_, name), file) in children.into_iter().take(MENU_LIMIT) {
            if let Some(file) = file {
                if let Some(path) = file.path.to_str() {
                    self.navigation(
                        result,
                        name,
                        Vim9Command::Open {
                            path: path.into(),
                            line: None,
                            column: None,
                        },
                    );
                }
            } else {
                result.entries.push(Choice {
                    key: format!("{prefix}{name}"),
                    title: format!("{name}/"),
                    directory: true,
                    unavailable: None,
                });
            }
        }
    }
}
