//! In-memory, single-window authority for a document replacement. Never persisted.
use super::{valid_identity, Attachment};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

const LIFETIME: Duration = Duration::from_secs(30);
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct View {
    version: u8,
    tabs: Vec<Tab>,
    split_enabled: bool,
    split_epochs: Vec<u32>,
    detached_local: Option<Attachment>,
    remote_draft: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Tab {
    owner: Attachment,
    shell: String,
    pane_id: Option<String>,
    geometry: Option<Geometry>,
    draft: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Geometry {
    rows: u16,
    cols: u16,
}
impl View {
    pub(super) fn validate(&self) -> Result<(), String> {
        let valid_owner =
            |v: &Attachment| valid_identity(&v.session) && v.epoch > 0 && v.generation > 0;
        let draft = |v: &str| v.len() <= 16384 && !v.contains('\0');
        let mut epochs = HashSet::new();
        let mut sessions = HashSet::new();
        let mut panes = HashSet::new();
        let valid = self.version == 1
            && self.tabs.len() <= 8
            && draft(&self.remote_draft)
            && self.tabs.iter().all(|tab| {
                valid_owner(&tab.owner)
                    && epochs.insert(tab.owner.epoch)
                    && sessions.insert(tab.owner.session.as_str())
                    && matches!(tab.shell.as_str(), "plain" | "jsh")
                    && draft(&tab.draft)
                    && tab.pane_id.as_ref().is_none_or(|id| {
                        tab.shell == "jsh"
                            && id.len() == 32
                            && id
                                .bytes()
                                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                            && panes.insert(id)
                    })
                    && tab
                        .geometry
                        .as_ref()
                        .is_none_or(|g| (1..=128).contains(&g.rows) && (1..=256).contains(&g.cols))
            })
            && self.split_epochs.len() == if self.split_enabled { 2 } else { 0 }
            && self.split_epochs.iter().all(|epoch| epochs.contains(epoch))
            && (self.split_epochs.len() != 2 || self.split_epochs[0] != self.split_epochs[1])
            && self
                .detached_local
                .as_ref()
                .is_none_or(|v| valid_owner(v) && self.tabs.iter().any(|tab| tab.owner == *v));
        if valid {
            Ok(())
        } else {
            Err("Invalid retained terminal layout.".into())
        }
    }
}
pub(super) struct Pending {
    pub(super) ticket: String,
    pub(super) owner: Attachment,
    pub(super) view: View,
    window: String,
    expires: Instant,
}
impl Pending {
    pub(super) fn new(owner: Attachment, view: View, window: &str, now: Instant) -> Self {
        Self {
            ticket: uuid::Uuid::new_v4().simple().to_string(),
            owner,
            view,
            window: window.into(),
            expires: now + LIFETIME,
        }
    }
    pub(super) fn expired(&self, now: Instant) -> bool {
        now >= self.expires
    }
    pub(super) fn authorize(
        &self,
        ticket: &str,
        window: &str,
        owner: &Attachment,
        read_only: bool,
        uncertain: bool,
        now: Instant,
    ) -> Result<(), String> {
        if self.expired(now)
            || ticket != self.ticket
            || window != self.window
            || owner != &self.owner
            || read_only
            || uncertain
        {
            Err("The one-use SSH writer handoff is unavailable. Reconnect view-only.".into())
        } else {
            Ok(())
        }
    }
}
// Reader metadata is not a writer capability. Keep one bounded snapshot for a
// short recovery window, tied to the original native window and SSH actor.
pub(super) struct RetainedLayout {
    owner: Attachment,
    window: String,
    view: View,
    expires: Instant,
}
impl Pending {
    pub(super) fn belongs_to(&self, window: &str) -> bool {
        self.window == window
    }
    pub(super) fn retained_layout(&self) -> RetainedLayout {
        let mut view = self.view.clone();
        // Never restore an unsent remote command into an unrelated local input.
        view.remote_draft.clear();
        RetainedLayout {
            owner: self.owner.clone(),
            window: self.window.clone(),
            view,
            expires: self.expires + Duration::from_secs(270),
        }
    }
}
impl RetainedLayout {
    pub(super) fn matches(&self, window: &str, current: &Attachment) -> bool {
        self.window == window
            && self.owner.session == current.session
            && self.owner.epoch == current.epoch
            && current.generation >= self.owner.generation
    }
    pub(super) fn take(
        self,
        window: &str,
        current: &Attachment,
        now: Instant,
    ) -> Result<View, String> {
        if !self.matches(window, current) || now >= self.expires {
            return Err(
                "Retained local layout is unavailable for this window and SSH actor.".into(),
            );
        }
        Ok(self.view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner() -> Attachment {
        Attachment {
            session: "a".repeat(32),
            epoch: 7,
            generation: 3,
        }
    }
    fn view() -> View {
        serde_json::from_value(serde_json::json!({"version":1,"tabs":[],"splitEnabled":false,"splitEpochs":[],"detachedLocal":null,"remoteDraft":""})).unwrap()
    }
    #[test]
    fn ssh_reload_authority_is_one_use_same_window_exact_generation_and_bounded() {
        let now = Instant::now();
        let mut slot = Some(Pending::new(owner(), view(), "main", now));
        let p = slot.take().unwrap();
        assert!(slot.take().is_none());
        assert!(p
            .authorize(&p.ticket, "main", &owner(), false, false, now)
            .is_ok());
        for (ticket, window, read_only, uncertain) in [
            ("wrong", "main", false, false),
            (&p.ticket, "other", false, false),
            (&p.ticket, "main", true, false),
            (&p.ticket, "main", false, true),
        ] {
            assert!(p
                .authorize(ticket, window, &owner(), read_only, uncertain, now)
                .is_err());
        }
        let mut stale = owner();
        stale.generation += 1;
        assert!(p
            .authorize(&p.ticket, "main", &stale, false, false, now)
            .is_err());
        assert!(p
            .authorize(&p.ticket, "main", &owner(), false, false, now + LIFETIME)
            .is_err());
    }
    #[test]
    fn ssh_reload_layout_rejects_duplicates_geometry_and_excess_drafts() {
        assert!(view().validate().is_ok());
        let mut v = view();
        v.remote_draft = "x".repeat(16385);
        assert!(v.validate().is_err());
        let row = serde_json::json!({"owner":owner(),"shell":"plain","paneId":null,"geometry":{"rows":24,"cols":80},"draft":"kept"});
        let mut v:View=serde_json::from_value(serde_json::json!({"version":1,"tabs":[row.clone(),row],"splitEnabled":false,"splitEpochs":[],"detachedLocal":null,"remoteDraft":""})).unwrap();
        assert!(v.validate().is_err());
        v.tabs.pop();
        assert!(v.validate().is_ok());
        v.tabs[0].geometry.as_mut().unwrap().cols = 257;
        assert!(v.validate().is_err());
    }
}

#[cfg(test)]
mod retained_tests {
    use super::*;
    #[test]
    fn ssh_reload_reader_metadata_has_no_writer_grant_and_expires_without_refresh() {
        let now = Instant::now();
        let owner = Attachment {
            session: "a".repeat(32),
            epoch: 3,
            generation: 5,
        };
        let view:View=serde_json::from_value(serde_json::json!({"version":1,"tabs":[],"splitEnabled":false,"splitEpochs":[],"detachedLocal":null,"remoteDraft":"must not insert"})).unwrap();
        let pending = Pending::new(owner.clone(), view, "main", now);
        assert!(pending
            .retained_layout()
            .take("main", &owner, now)
            .unwrap()
            .remote_draft
            .is_empty());
        assert!(pending
            .retained_layout()
            .take("other", &owner, now)
            .is_err());
        let mut foreign = owner.clone();
        foreign.session = "b".repeat(32);
        assert!(pending
            .retained_layout()
            .take("main", &foreign, now)
            .is_err());
        foreign = owner.clone();
        foreign.generation -= 1;
        assert!(pending
            .retained_layout()
            .take("main", &foreign, now)
            .is_err());
        assert!(pending
            .retained_layout()
            .take("main", &owner, now + Duration::from_secs(300))
            .is_err());
    }
}
