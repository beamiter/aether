//! Scoped, revision-qualified evidence search through the shared Workspace Tools.
use super::native_tasks;
use jterm_core::agent_workspace::{
    controllers::tools::WorkspaceTools, ConversationId, WorkspaceSearchHit, WorkspaceTextDocument,
};
use std::{
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Anchor {
    conversation: ConversationId,
    event_id: String,
    revision: String,
}

impl Anchor {
    pub(super) fn of(hit: &WorkspaceSearchHit) -> Self {
        Self {
            conversation: hit.row.id.clone(),
            event_id: hit.anchor.event_id.clone(),
            revision: hit.anchor.revision.clone(),
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, tag = "operation", rename_all = "snake_case")]
pub(crate) enum Choice {
    Search {
        query: String,
        include_archived: bool,
        scope: Option<native_tasks::Identity>,
    },
    More {
        generation: u32,
        before: String,
        scope: Option<native_tasks::Identity>,
    },
    Open {
        generation: u32,
        anchor: Anchor,
        scope: Option<native_tasks::Identity>,
    },
}

impl Choice {
    pub(super) fn scope(&self) -> Option<&native_tasks::Identity> {
        match self {
            Self::Search { scope, .. } | Self::More { scope, .. } | Self::Open { scope, .. } => {
                scope.as_ref()
            }
        }
    }
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Reply {
    Page { page: Page },
    Document { generation: u32, document: Document },
}

#[derive(serde::Serialize)]
pub(crate) struct Page {
    generation: u32,
    scope: Option<ConversationId>,
    query: String,
    hits: Vec<Hit>,
    next_before: Option<String>,
    coverage: String,
}

#[derive(serde::Serialize)]
pub(super) struct Hit {
    anchor: Anchor,
    title: String,
    source: &'static str,
    snippet: String,
    truncated: bool,
}

impl From<&WorkspaceSearchHit> for Hit {
    fn from(hit: &WorkspaceSearchHit) -> Self {
        Self {
            anchor: Anchor::of(hit),
            title: hit.row.title.clone(),
            source: hit.source.label(),
            snippet: hit.snippet.clone(),
            truncated: hit.truncated,
        }
    }
}

#[derive(serde::Serialize)]
pub(crate) struct Document {
    anchor: Anchor,
    source: &'static str,
    text: String,
    truncated: bool,
}

#[derive(Default)]
pub(super) struct Tools {
    shared: WorkspaceTools,
    generation: u32,
    ready: bool,
    scope: Option<native_tasks::Identity>,
    conversation: Option<ConversationId>,
}

impl Tools {
    pub(super) fn execute(
        &mut self,
        choice: Choice,
        conversation: Option<ConversationId>,
    ) -> Result<Reply, String> {
        match choice {
            Choice::Search {
                query,
                include_archived,
                scope,
            } => {
                let generation = self
                    .generation
                    .checked_add(1)
                    .ok_or("Tools search generation exhausted")?;
                self.shared.close_document();
                self.shared.set_search("", None, false);
                self.shared
                    .set_search(&query, conversation.clone(), include_archived);
                self.generation = generation;
                self.ready = false;
                self.scope = scope;
                self.conversation = conversation;
                self.confirm(|shared| shared.searching())?;
                self.ready = true;
                Ok(self.page())
            }
            Choice::More {
                generation,
                before,
                scope,
            } => {
                self.current(generation, &scope, &conversation)?;
                if self
                    .shared
                    .page
                    .next_before
                    .map(|value| value.to_string())
                    .as_ref()
                    != Some(&before)
                {
                    return Err(
                        "Search page changed. Search again before requesting older results.".into(),
                    );
                }
                let old_len = self.shared.page.hits.len();
                self.shared.load_more();
                self.confirm(|shared| shared.searching())?;
                self.shared.page.hits.drain(..old_len);
                Ok(self.page())
            }
            Choice::Open {
                generation,
                anchor,
                scope,
            } => {
                self.current(generation, &scope, &conversation)?;
                let hit = self
                    .shared
                    .page
                    .hits
                    .iter()
                    .find(|hit| Anchor::of(hit) == anchor)
                    .ok_or("Choose a result from the current search page")?
                    .clone();
                self.shared.open_hit(hit);
                self.confirm(|shared| shared.document_loading)?;
                let document = self
                    .shared
                    .document
                    .as_ref()
                    .ok_or("Missing exact Workspace evidence")?;
                Ok(Reply::Document {
                    generation,
                    document: Document::exact(document, anchor)?,
                })
            }
        }
    }

    fn current(
        &self,
        generation: u32,
        scope: &Option<native_tasks::Identity>,
        conversation: &Option<ConversationId>,
    ) -> Result<(), String> {
        if !self.ready
            || generation != self.generation
            || scope != &self.scope
            || conversation != &self.conversation
        {
            return Err(
                "Tools search selection changed. Search again to open current evidence.".into(),
            );
        }
        Ok(())
    }

    fn confirm(&mut self, pending: impl Fn(&WorkspaceTools) -> bool) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            self.shared.poll();
            if !pending(&self.shared) {
                return self.shared.notice.take().map_or(Ok(()), Err);
            }
            if Instant::now() >= deadline {
                self.ready = false;
                self.shared = WorkspaceTools::default();
                return Err("The Workspace host did not confirm the search operation.".into());
            }
            thread::sleep(Duration::from_millis(12));
        }
    }

    fn page(&self) -> Reply {
        Reply::Page {
            page: Page {
                generation: self.generation,
                scope: self.conversation.clone(),
                query: self.shared.query().to_owned(),
                hits: self.shared.page.hits.iter().map(Hit::from).collect(),
                next_before: self.shared.page.next_before.map(|value| value.to_string()),
                coverage: self.shared.page.coverage.label(),
            },
        }
    }
}

impl Document {
    pub(super) fn exact(
        document: &WorkspaceTextDocument,
        anchor: Anchor,
    ) -> Result<Document, String> {
        if document.conversation != anchor.conversation
            || document.event_id != anchor.event_id
            || document.revision != anchor.revision
        {
            return Err("Workspace returned a different evidence revision".into());
        }
        Ok(Document {
            anchor,
            source: document.source.label(),
            text: document.text.clone(),
            truncated: document.truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_current_search_can_admit_a_saved_anchor() {
        let mut tools = Tools {
            generation: 8,
            ready: true,
            ..Tools::default()
        };
        let anchor = Anchor {
            conversation: ConversationId("fixture".into()),
            event_id: "validation:fixture".into(),
            revision: "a".into(),
        };
        for (generation, expected) in [(7, "selection changed"), (8, "current search page")] {
            let result = tools.execute(
                Choice::Open {
                    generation,
                    anchor: anchor.clone(),
                    scope: None,
                },
                None,
            );
            assert!(matches!(result, Err(error) if error.contains(expected)));
        }
    }
}
