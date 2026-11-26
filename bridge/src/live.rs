//! Attachment and frame publication over the shared Linux runtime.
use crate::{
    from_engine,
    history_search::{HistorySearch, HistorySearchRequest, HistorySearchResponse},
    protocol::MAX_HISTORY_SNAPSHOT_BYTES,
    protocol_v4::{HistorySource, MAX_PROJECTED_HISTORY_ROWS},
    semantic::{self, Attachment, Resume, SessionSelector, Update},
    SnapshotFormat, WireFrame,
};
use jterm_engine::{
    frame::CellFrame,
    frame_retention::{process_frame_budget, RetainedCellFrame},
    history_reflow::HistoryAnchor,
    terminal::{HistoryPage, Terminal},
};
use jterm_headless_session_spike::{Session, WakeSink};
use std::collections::BTreeMap;
mod command_blocks;
mod raw_suffix;
pub use crate::protocol::{CloseStatus, HistorySnapshot, SessionStatus};
pub use command_blocks::{
    BlockSelection, CommandActivity, CommandBlockSnapshot, CompletedPaneHistory, RetainedBlock,
};
use raw_suffix::RawSuffix;

const HISTORY_PAGE_ROWS: usize = 128;
const MIN_HISTORY_PAGE_ROWS: usize = 32;
const MAX_HISTORY_OFFSET: usize = 1024;
// Match the web canvas's intrinsic cell geometry. Device scaling is applied
// only by its compositor and does not change the terminal's cell dimensions.
const CELL_PIXELS: (u16, u16) = (12, 24);

#[derive(Default)]
pub struct LiveBridge {
    session: Option<Session>,
    id: String,
    epoch: u32,
    generation: u32,
    input_attached: bool,
    input_waits_for_snapshot: bool,
    resumed_from: Option<u32>,
    flight: Option<RetainedCellFrame>,
    accepted: Option<RetainedCellFrame>,
    local_desktop: bool,
    inline_graphics: bool,
    unpublished_background: bool,
    hyperlink_policy: Option<jterm_engine::hyperlink::HyperlinkPolicy>,
    stable: Option<RetainedCellFrame>,
    #[cfg(test)]
    frame_budget_override:
        Option<std::sync::Arc<jterm_engine::retained_bytes::RetainedBytesBudget>>,
    history_search: Option<HistorySearch>,
    submitted_chat_prompt: Option<crate::protocol_v4::Counter>,
    command_bookmarks: BTreeMap<String, String>,
    command_source_stream: Option<String>,
    restored_commands: command_blocks::RestoredCommands,
    raw: RawSuffix<Attachment>,
}
impl LiveBridge {
    /// Use only for the local desktop owner that reports sticky write health.
    /// Standalone/SSH transport retains its existing recovery contract.
    #[doc(hidden)]
    pub fn local_desktop() -> Self {
        Self::local_desktop_after(0)
    }

    /// Desktop owner allocates a fresh epoch across all of its retained PTYs;
    /// this stores the last allocated value, not an attachment capability.
    #[doc(hidden)]
    pub fn local_desktop_after(last_epoch: u32) -> Self {
        Self {
            epoch: last_epoch,
            local_desktop: true,
            ..Self::default()
        }
    }

    pub fn set_hyperlink_policy(&mut self, policy: jterm_engine::hyperlink::HyperlinkPolicy) {
        self.hyperlink_policy = Some(policy);
    }

    pub fn enable_inline_graphics(&mut self, format: SnapshotFormat) {
        if !self.local_desktop || format != SnapshotFormat::ChannelImages || self.inline_graphics {
            return;
        }
        self.inline_graphics = true;
        if let Some(session) = &mut self.session {
            session.enable_inline_graphics(CELL_PIXELS, crate::inline_png::decode);
        }
    }

    /// Wake the desktop worker after the reader queues output or an exit.
    pub fn notify_on_output(
        &self,
        notify: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), String> {
        self.session
            .as_ref()
            .ok_or("terminal session is closed")?
            .set_output_wake(WakeSink::new(notify))
    }

    pub fn open(&mut self, rows: usize, cols: usize) -> Result<u32, String> {
        self.open_shell(rows, cols, false)
    }

    pub fn open_integrated(&mut self, rows: usize, cols: usize) -> Result<u32, String> {
        self.open_shell(rows, cols, true)
    }

    fn open_shell(&mut self, rows: usize, cols: usize, integrated: bool) -> Result<u32, String> {
        self.open_session(|epoch| Session::spawn_family_with_epoch(rows, cols, epoch, integrated))
    }

    /// Launch a dedicated local editor from a host-authorized argv and cwd.
    /// A fresh bridge's first epoch matches the prepared Session's epoch.
    pub fn open_prepared_editor(
        &mut self,
        rows: usize,
        cols: usize,
        argv: &[std::ffi::OsString],
        cwd: &std::path::Path,
        env: &[(std::ffi::OsString, std::ffi::OsString)],
    ) -> Result<u32, String> {
        if self.epoch != 0 {
            return Err("A prepared editor requires a fresh terminal owner.".into());
        }
        self.open_session(|_| Session::spawn_command(rows, cols, argv, Some(cwd), env))
    }

    fn open_session(
        &mut self,
        spawn: impl FnOnce(std::num::NonZeroU64) -> Result<Session, String>,
    ) -> Result<u32, String> {
        if self.session.is_some() {
            return Err("close the existing session first".into());
        }
        let epoch = self.epoch.checked_add(1).ok_or("epoch exhausted")?;
        let id = semantic::session_id()?;
        let command_source_stream = self.local_desktop.then(semantic::session_id).transpose()?;
        let frame_epoch = std::num::NonZeroU64::new(u64::from(epoch)).ok_or("invalid epoch")?;
        let mut session = spawn(frame_epoch)?;
        session.enable_ordered_input();
        if self.local_desktop {
            session.enable_underline_colors();
            session.enable_grapheme_clusters();
            session.enable_keyboard_disambiguation();
            session.enable_history_preserving_resize();
        }
        if self.inline_graphics {
            session.enable_inline_graphics(CELL_PIXELS, crate::inline_png::decode);
        }
        if let Some(policy) = self.hyperlink_policy {
            session.enable_hyperlinks(policy);
        }
        self.record_stable(session.terminal().frame()?);
        self.unpublished_background = false;
        self.epoch = epoch;
        self.id = id;
        self.generation = 1;
        self.input_attached = true;
        self.input_waits_for_snapshot = false;
        self.resumed_from = None;
        self.session = Some(session);
        self.flight = None;
        self.accepted = None;
        self.history_search = None;
        self.submitted_chat_prompt = None;
        self.command_source_stream = command_source_stream;
        self.restored_commands = Default::default();
        self.raw = RawSuffix::default();
        Ok(epoch)
    }
    fn check(&self, epoch: u32) -> Result<(), String> {
        if epoch != self.epoch || self.session.is_none() {
            Err("stale or closed session".into())
        } else {
            Ok(())
        }
    }
    pub fn attachment(&self, epoch: u32) -> Result<Attachment, String> {
        self.check(epoch)?;
        Ok(Attachment {
            session: self.id.clone(),
            epoch,
            generation: self.generation,
        })
    }

    fn validate_session_identity(&self, session: &str, epoch: u32) -> Result<u32, String> {
        self.check(epoch)?;
        if session != self.id {
            return Err("foreign terminal session".into());
        }
        Ok(epoch)
    }

    pub fn validate_session(&self, owner: &Attachment) -> Result<u32, String> {
        self.validate_session_identity(&owner.session, owner.epoch)
    }

    /// Attach a read-only view to the selected PTY, never another retained one.
    /// An omitted selector preserves explicit singleton attach behavior.
    pub fn attach_read_only(
        &mut self,
        target: Option<&SessionSelector>,
    ) -> Result<Option<Resume>, String> {
        self.attach_read_only_with(target, from_engine)
    }

    pub fn attach_read_only_with<F>(
        &mut self,
        target: Option<&SessionSelector>,
        encode: impl FnOnce(&CellFrame, &str, u32) -> Result<F, String>,
    ) -> Result<Option<Resume<F>>, String> {
        if let Some(target) = target {
            self.validate_session_identity(&target.session, target.epoch)?;
        }
        self.resume_with(None, encode)
    }

    pub fn validate_attachment(&self, owner: &Attachment) -> Result<u32, String> {
        let epoch = self.validate_session(owner)?;
        if owner.generation != self.generation {
            return Err("stale terminal attachment".into());
        }
        Ok(epoch)
    }

    pub fn has_acknowledged_frame(&self) -> bool {
        self.accepted.is_some()
    }

    pub fn validate_input(&self, owner: &Attachment) -> Result<u32, String> {
        let epoch = self.validate_attachment(owner)?;
        if !self.input_attached {
            return Err("reconnected terminal view is read-only".into());
        }
        if self.input_waits_for_snapshot && self.accepted.is_none() {
            return Err("acknowledge the local tab snapshot before sending input".into());
        }
        Ok(epoch)
    }

    pub fn next_detached_generation(&self, owner: &Attachment) -> Result<u32, String> {
        self.validate_attachment(owner)?;
        self.generation
            .checked_add(1)
            .ok_or_else(|| "attachment exhausted".into())
    }

    pub fn validate_logical_input(&self, owner: &Attachment) -> Result<u32, String> {
        let epoch = self.validate_input(owner)?;
        if !self.raw.is_idle() {
            return Err("finish the pending raw input event first".into());
        }
        Ok(epoch)
    }

    /// Order an editor-channel action after input already admitted to its PTY.
    pub fn flush_editor_input(&mut self, owner: &Attachment) -> Result<(), String> {
        self.validate_logical_input(owner)?;
        if !self.session.as_ref().unwrap().input_idle() {
            self.pump_unpublished()?;
        }
        let session = self.session.as_ref().unwrap();
        if session.is_exited() || session.write_error().is_some() {
            return Err("The editor PTY can no longer deliver input.".into());
        }
        if !session.input_idle() {
            return Err("Vim is still receiving terminal input. Retry the editor action.".into());
        }
        Ok(())
    }

    pub(crate) fn recovered_input_allowed(&self, owner: &Attachment) -> bool {
        self.validate_logical_input(owner).is_ok()
            && self
                .session
                .as_ref()
                .is_some_and(|session| !session.is_exited() && session.write_error().is_none())
    }

    /// Only the desktop owner may park an already-writable tab. Queued input,
    /// partial writes, and known write failures cannot acquire another lease.
    pub fn can_park_local_input(&self, owner: &Attachment) -> bool {
        self.validate_input(owner).is_ok() && self.local_input_idle()
    }

    fn local_input_idle(&self) -> bool {
        self.local_desktop
            && self.raw.is_idle()
            && self.session.as_ref().is_some_and(|session| {
                session.input_idle() && !session.is_exited() && session.write_error().is_none()
            })
    }

    pub fn can_resume_local_input(&self) -> bool {
        self.input_attached && self.local_input_idle()
    }

    /// This publication is scoped to the local desktop tab registry. A
    /// reconnect outside that registry continues to be read-only.
    pub fn activate_local_tab(
        &mut self,
        target: &SessionSelector,
        grant_input: bool,
    ) -> Result<(Resume, bool), String> {
        self.activate_local_tab_with(target, grant_input, from_engine)
    }

    pub fn activate_local_tab_with<F>(
        &mut self,
        target: &SessionSelector,
        grant_input: bool,
        encode: impl FnOnce(&CellFrame, &str, u32) -> Result<F, String>,
    ) -> Result<(Resume<F>, bool), String> {
        let view = self
            .attach_read_only_with(Some(target), encode)?
            .ok_or("terminal session closed")?;
        let writable = grant_input && self.local_input_idle();
        self.input_attached = writable;
        self.input_waits_for_snapshot = writable;
        Ok((view, writable))
    }

    /// Retrying the exact unacknowledged local activation returns the same
    /// publication. Once acknowledged or replaced, the old view has no grant.
    pub fn pending_local_tab(&self, owner: &Attachment) -> Result<WireFrame, String> {
        self.pending_local_tab_with(owner, from_engine)
    }

    pub fn pending_local_tab_with<F>(
        &self,
        owner: &Attachment,
        encode: impl FnOnce(&CellFrame, &str, u32) -> Result<F, String>,
    ) -> Result<F, String> {
        self.validate_attachment(owner)?;
        if self.accepted.is_some() {
            return Err("local tab activation already acknowledged".into());
        }
        let frame = self
            .flight
            .as_ref()
            .ok_or("no pending local tab snapshot")?;
        encode(frame, &self.id, self.epoch)
    }

    pub fn detach(&mut self, owner: &Attachment) -> Result<(), String> {
        self.generation = self.next_detached_generation(owner)?;
        self.session.as_mut().unwrap().retire_pending_input();
        self.raw.retire_view();
        self.input_attached = false;
        self.flight = None;
        self.accepted = None;
        self.history_search = None;
        self.resumed_from = None;
        Ok(())
    }

    /// A reconnect starts with a snapshot and no dependency on the old viewer's
    /// data. A retry of an unacknowledged recovery gets the same attachment.
    pub fn resume(&mut self, expected: Option<&Attachment>) -> Result<Option<Resume>, String> {
        self.resume_with(expected, from_engine)
    }

    pub fn resume_with<F>(
        &mut self,
        expected: Option<&Attachment>,
        encode: impl FnOnce(&CellFrame, &str, u32) -> Result<F, String>,
    ) -> Result<Option<Resume<F>>, String> {
        if let Some(owner) = expected {
            self.validate_session(owner)?;
            if self.resumed_from == Some(owner.generation) && self.accepted.is_none() {
                if let Some(frame) = self.flight.as_ref() {
                    return Ok(Some(Resume {
                        owner: self.attachment(self.epoch)?,
                        frame: encode(frame, &self.id, self.epoch)?,
                    }));
                }
            }
            self.validate_attachment(owner)?;
        } else if self.session.is_none() {
            return Ok(None);
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or("attachment exhausted")?;
        let frame = self.recover_frame_with(self.epoch, encode)?;
        self.generation = generation;
        let session = self.session.as_mut().unwrap();
        if expected.is_none() {
            session.retire_pending_input();
            self.input_attached = false;
        } else if session.retire_partial_input() {
            self.input_attached = false;
        }
        self.raw.retire_view();
        self.resumed_from = expected.map(|owner| owner.generation);
        self.accepted = None;
        self.history_search = None;
        Ok(Some(Resume {
            owner: self.attachment(self.epoch)?,
            frame,
        }))
    }

    fn poll_io(&mut self, capture: bool) -> Result<usize, String> {
        let session = self.session.as_mut().unwrap();
        let result = session.drive_with_sync_capture(capture);
        if session.write_error().is_some() {
            self.input_attached = false;
        }
        result
    }

    /// A local desktop owner may drain a detached PTY after witnessing that
    /// no raw input transaction or reader is active under its registry lock.
    /// Never consumes logical input or touches frame publication/ACK state.
    #[doc(hidden)]
    pub fn pump_detached_idle(&mut self) -> Result<bool, String> {
        if !self.local_desktop || self.input_attached {
            return Ok(false);
        }
        match &mut self.session {
            Some(session) => session.poll_detached_idle(),
            None => Ok(false),
        }
    }

    /// Keep a local attached PTY advancing while publication is backpressured.
    pub fn pump_attached_io(&mut self, epoch: u32) -> Result<bool, String> {
        self.check(epoch)?;
        self.poll_io(false)?;
        Ok(!self.session.as_ref().unwrap().is_drained())
    }

    /// Advance the owner while no receiver is issuing publications. Retain
    /// the last paint boundary across synchronized-output holds.
    pub fn pump_unpublished(&mut self) -> Result<bool, String> {
        self.pump_unpublished_with_activity()
            .map(|(active, _)| active)
    }

    /// Report whether the PTY can still produce output and how many bytes were read.
    pub(crate) fn pump_unpublished_with_activity(&mut self) -> Result<(bool, usize), String> {
        let Some(session) = self.session.as_ref() else {
            return Ok((false, 0));
        };
        if session.is_drained() {
            return Ok((false, 0));
        }
        self.unpublished_background = true;
        let output_bytes = self.poll_io(true)?;
        let session = self.session.as_mut().unwrap();
        let completed = session.take_completed_sync_frame();
        let active = !session.is_drained();
        let frame = if !active || !session.terminal().publication_held() {
            Some(session.terminal().frame()?)
        } else {
            completed
        };
        if let Some(frame) = frame {
            self.record_stable(frame);
        }
        Ok((active, output_bytes))
    }

    pub fn has_unacknowledged_frame(&self) -> bool {
        self.flight.is_some()
    }

    /// Resolve a click only against the exact acknowledged publication.
    pub fn acknowledged_hyperlink(
        &self,
        revision: u64,
        row: usize,
        mut col: usize,
    ) -> Result<&str, String> {
        let frame = self
            .accepted
            .as_ref()
            .filter(|frame| frame.stamp().revision == revision)
            .ok_or("terminal hyperlink frame changed")?;
        let line = frame
            .rows()
            .get(row)
            .ok_or("terminal hyperlink outside view")?;
        if line
            .cells
            .get(col)
            .is_some_and(|cell| cell.flags.wide_continuation())
        {
            col = col
                .checked_sub(1)
                .ok_or("terminal hyperlink outside view")?;
        }
        let cell = line
            .cells
            .get(col)
            .ok_or("terminal hyperlink outside view")?;
        frame
            .hyperlink_target(cell.hyperlink_id)
            .ok_or_else(|| "no hyperlink at this cell".into())
    }

    fn poll_frame(&mut self, epoch: u32, snapshot: bool) -> Result<Option<CellFrame>, String> {
        self.check(epoch)?;
        self.poll_io(self.flight.is_none())?;
        let session = self.session.as_mut().unwrap();
        if self.flight.is_some() {
            return Ok(None);
        }
        let completed = session.take_completed_sync_frame();
        let frame = if !session.is_drained() && session.terminal().publication_held() {
            match completed {
                Some(frame) => frame,
                None if snapshot || self.unpublished_background => {
                    let Some(stable) = self.stable.as_deref() else {
                        return Ok(None);
                    };
                    stable.clone()
                }
                None => return Ok(None),
            }
        } else {
            session.terminal().frame()?
        };
        if !snapshot
            && self
                .accepted
                .as_ref()
                .is_some_and(|last| frame.stamp().revision == last.stamp().revision)
        {
            return Ok(None);
        }
        Ok(Some(frame))
    }

    fn record_stable(&mut self, frame: CellFrame) {
        let frame = match self.stable.as_mut() {
            Some(stable) if stable.stamp() == frame.stamp() => return,
            Some(stable) => match stable.try_replace(frame) {
                Ok(()) => return,
                Err(frame) => frame,
            },
            None => frame,
        };
        if let Ok(frame) = RetainedCellFrame::try_retain(frame, self.frame_budget()) {
            self.stable = Some(frame);
        }
    }

    fn frame_budget(&self) -> &std::sync::Arc<jterm_engine::retained_bytes::RetainedBytesBudget> {
        #[cfg(test)]
        if let Some(budget) = self.frame_budget_override.as_ref() {
            return budget;
        }
        process_frame_budget()
    }

    fn reserve_publication(&self, frame: CellFrame) -> Option<RetainedCellFrame> {
        if let Some(stable) = self
            .stable
            .as_ref()
            .filter(|stable| stable.stamp() == frame.stamp())
        {
            return Some(stable.clone());
        }
        RetainedCellFrame::try_retain(frame, self.frame_budget()).ok()
    }

    fn publish(&mut self, frame: RetainedCellFrame) {
        self.stable = Some(frame.clone());
        self.flight = Some(frame);
    }

    pub fn poll(&mut self, epoch: u32) -> Result<Option<WireFrame>, String> {
        let Some(frame) = self.poll_frame(epoch, false)? else {
            return Ok(None);
        };
        let Some(frame) = self.reserve_publication(frame) else {
            return Ok(None);
        };
        let wire = from_engine(&frame, &self.id, epoch)?;
        self.publish(frame);
        Ok(Some(wire))
    }

    pub fn poll_update(&mut self, epoch: u32) -> Result<Option<Update>, String> {
        let Some(frame) = self.poll_frame(epoch, false)? else {
            return Ok(None);
        };
        let Some(frame) = self.reserve_publication(frame) else {
            return Ok(None);
        };
        let update = semantic::update(self.accepted.as_deref(), &frame, &self.id, epoch)?;
        self.publish(frame);
        Ok(Some(update))
    }

    /// Start the Channel with its own full base even if a compatibility
    /// snapshot for the same state was already applied during tab activation.
    pub fn poll_channel_frame(
        &mut self,
        epoch: u32,
        snapshot: bool,
        format: SnapshotFormat,
    ) -> Result<Option<(crate::protocol_v4::Counter, String)>, String> {
        self.poll_channel_with_scratch(
            epoch,
            snapshot,
            format,
            crate::semantic_v4::FrameScratch::acquire(),
        )
    }

    fn poll_channel_with_scratch(
        &mut self,
        epoch: u32,
        snapshot: bool,
        format: SnapshotFormat,
        scratch: Option<crate::semantic_v4::FrameScratch>,
    ) -> Result<Option<(crate::protocol_v4::Counter, String)>, String> {
        let version = format
            .channel_version()
            .ok_or("terminal Channel version required")?;
        let Some(scratch) = scratch else {
            self.check(epoch)?;
            // Encoding is deferrable. Keep advancing the owner, including
            // its last completed synchronized paint boundary, and leave the
            // existing delivery/ack state alone.
            self.pump_unpublished()?;
            return Ok(None);
        };
        let Some(frame) = self.poll_frame(epoch, snapshot)? else {
            return Ok(None);
        };
        let Some(frame) = self.reserve_publication(frame) else {
            return Ok(None);
        };
        let previous = if snapshot {
            None
        } else {
            self.accepted.as_deref()
        };
        let update = scratch.encode(previous, &frame, &self.id, version)?;
        self.publish(frame);
        Ok(Some(update))
    }
    /// Explicit full-snapshot recovery after lost delivery/ACK. Never changes PTY or input.
    /// Caller must discard older outstanding transport callbacks before applying this frame.
    pub fn recover_frame(&mut self, epoch: u32) -> Result<WireFrame, String> {
        self.recover_frame_with(epoch, from_engine)
    }

    fn recover_frame_with<F>(
        &mut self,
        epoch: u32,
        encode: impl FnOnce(&CellFrame, &str, u32) -> Result<F, String>,
    ) -> Result<F, String> {
        self.check(epoch)?;
        let frame = self
            .reserve_publication(self.current_snapshot()?)
            .ok_or("terminal frame publication is temporarily busy")?;
        let wire = encode(&frame, &self.id, epoch)?;
        self.publish(frame);
        Ok(wire)
    }

    /// Read-only split observation. It neither advances the PTY nor changes
    /// the acknowledged base, recovery receipt, or input attachment.
    pub fn observe_frame(
        &self,
        epoch: u32,
        revision: Option<u32>,
    ) -> Result<Option<WireFrame>, String> {
        self.check(epoch)?;
        let frame = self.current_snapshot()?;
        if revision.is_some_and(|revision| u64::from(revision) >= frame.stamp().revision) {
            return Ok(None);
        }
        from_engine(&frame, &self.id, epoch).map(Some)
    }

    pub fn observe_channel_frame(
        &self,
        epoch: u32,
        revision: Option<crate::protocol_v4::Counter>,
        format: SnapshotFormat,
    ) -> Result<Option<(crate::protocol_v4::Counter, String)>, String> {
        let version = format
            .channel_version()
            .ok_or("terminal Channel version required")?;
        self.check(epoch)?;
        let Some(scratch) = crate::semantic_v4::FrameScratch::acquire() else {
            return Ok(None);
        };
        let frame = self.current_snapshot()?;
        if revision.is_some_and(|revision| revision.get() >= frame.stamp().revision) {
            return Ok(None);
        }
        scratch.encode(None, &frame, &self.id, version).map(Some)
    }

    fn current_snapshot(&self) -> Result<CellFrame, String> {
        let session = self.session.as_ref().unwrap();
        if !session.is_drained() && session.terminal().publication_held() {
            self.stable
                .as_deref()
                .cloned()
                .ok_or_else(|| "terminal frame publication is temporarily busy".into())
        } else {
            session.terminal().frame()
        }
    }
    /// Independent immutable history snapshot; does not consume or acknowledge a live frame.
    pub fn history(&self, epoch: u32, offset: usize) -> Result<HistorySnapshot, String> {
        self.check(epoch)?;
        let terminal = self.session.as_ref().unwrap().terminal();
        let page = bounded_history_page(terminal, offset)?;
        Ok(HistorySnapshot {
            epoch,
            revision: page.revision.to_string(),
            offset: page.offset,
            total_rows: page.total_rows,
            row_count: Some(page.row_count),
            text: page.text,
        })
    }

    /// Paint and copy one bounded page from the shared primary-history layout.
    pub fn history_projection(
        &self,
        epoch: u32,
        offset: usize,
        anchor: Option<HistorySource>,
        shift: i32,
    ) -> Result<String, String> {
        let owner = self.attachment(epoch)?;
        let terminal = self.session.as_ref().unwrap().terminal();
        let (layout, first, page) = projected_history_page(terminal, offset, anchor, shift)?;
        crate::semantic_v4::encode_history(
            owner,
            &page,
            crate::semantic_v4::history_columns(&page, layout.columns()),
            page.revision,
            &page.text,
            |row| layout.anchor_at(first + row),
        )
    }

    /// Oldest accessible archive identity after eviction or explicit clear.
    pub fn history_oldest_row(
        &self,
        epoch: u32,
    ) -> Result<Option<crate::protocol_v4::Counter>, String> {
        self.check(epoch)?;
        let session = self.session.as_ref().unwrap();
        let terminal = session.terminal();
        if !self.local_desktop
            || terminal.alternate_screen_active()
            || (!session.is_drained() && terminal.publication_held())
        {
            return Ok(None);
        }
        Ok(terminal
            .history_extent()
            .start
            .checked_add(1)
            .and_then(crate::protocol_v4::Counter::new))
    }

    pub fn search_history(
        &mut self,
        epoch: u32,
        request: HistorySearchRequest,
        format: SnapshotFormat,
    ) -> Result<HistorySearchResponse, String> {
        self.check(epoch)?;
        request.validate()?;
        let owner = (format == SnapshotFormat::ChannelImages)
            .then(|| self.attachment(epoch))
            .transpose()?;
        if request.restart {
            if self
                .history_search
                .as_ref()
                .is_some_and(|search| search.id >= request.id)
            {
                return Err("stale history search".into());
            }
            self.history_search = Some(HistorySearch::new(request.id, request.query.clone())?);
        }
        let search = self
            .history_search
            .as_mut()
            .filter(|search| search.id == request.id && search.query() == request.query)
            .ok_or("start a new history search")?;
        search.step(self.session.as_ref().unwrap().terminal(), epoch, owner)
    }
    pub fn status(&self, epoch: u32) -> Result<SessionStatus, String> {
        self.check(epoch)?;
        let session = self.session.as_ref().unwrap();
        Ok(SessionStatus {
            epoch,
            exited: session.is_exited(),
            drained: session.is_drained(),
            write_error: session.write_error().map(str::to_owned),
            exit_output_notice: session
                .exit_status()
                .and_then(|status| status.tail.notice())
                .map(str::to_owned),
            dropped_reply_bytes: session.dropped_reply_bytes().to_string(),
            unsupported: session.terminal().unsupported_count().to_string(),
            command_blocks_enabled: session.terminal().completed_command_blocks().is_some(),
        })
    }
    pub fn acknowledge(&mut self, epoch: u32, revision: u32) -> Result<(), String> {
        self.acknowledge_publication(epoch, u64::from(revision))
    }

    pub fn acknowledge_publication(&mut self, epoch: u32, revision: u64) -> Result<(), String> {
        self.check(epoch)?;
        if self.flight.is_none()
            && self
                .accepted
                .as_ref()
                .is_some_and(|frame| frame.stamp().revision == revision)
        {
            return Ok(()); // retry of a successfully applied but lost ACK response
        }
        if self
            .flight
            .as_ref()
            .is_none_or(|frame| frame.stamp().revision != revision)
        {
            return Err("wrong frame ACK".into());
        }
        self.accepted = self.flight.take();
        self.resumed_from = None;
        Ok(())
    }
    fn enqueue(&mut self, epoch: u32, bytes: Vec<u8>) -> Result<(), String> {
        self.check(epoch)?;
        self.session.as_mut().unwrap().submit_input(bytes)
    }
    pub fn key(
        &mut self,
        epoch: u32,
        name: &str,
        shift: bool,
        alt: bool,
        ctrl: bool,
    ) -> Result<(), String> {
        self.check(epoch)?;
        let bytes = self
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .encode_key(name, shift, alt, ctrl)?;
        self.enqueue(epoch, bytes)
    }
    pub fn text(&mut self, epoch: u32, text: &str) -> Result<(), String> {
        self.check(epoch)?;
        let bytes = self
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .encode_text(text)?;
        self.enqueue(epoch, bytes)
    }
    pub fn paste(&mut self, epoch: u32, text: &str) -> Result<(), String> {
        self.check(epoch)?;
        let bytes = self
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .encode_paste(text)?;
        self.enqueue(epoch, bytes)
    }
    pub fn write_owned_raw(&mut self, owner: Attachment, bytes: Vec<u8>) -> Result<usize, String> {
        self.validate_input(&owner)?;
        let session = self.session.as_mut().unwrap();
        self.raw
            .write(owner, bytes, |bytes| session.write_transaction_chunk(bytes))
    }

    #[cfg(test)]
    fn write(&mut self, epoch: u32, text: &[u8]) -> Result<usize, String> {
        self.write_owned_raw(self.attachment(epoch)?, text.to_vec())
    }
    pub fn resize(&mut self, epoch: u32, rows: usize, cols: usize) -> Result<(), String> {
        self.check(epoch)?;
        self.session.as_mut().unwrap().resize(rows, cols)
    }
    pub fn close(&mut self, epoch: u32) -> Result<CloseStatus, String> {
        self.check(epoch)?;
        self.session.as_mut().unwrap().close()?;
        let cleanup_pending = self.session.as_ref().unwrap().cleanup_pending();
        self.session.take();
        self.id.clear();
        self.input_attached = false;
        self.stable = None;
        self.flight = None;
        self.accepted = None;
        self.resumed_from = None;
        self.history_search = None;
        self.command_bookmarks.clear();
        self.command_source_stream = None;
        self.restored_commands = Default::default();
        self.raw = RawSuffix::default();
        Ok(CloseStatus { cleanup_pending })
    }
}

fn projected_history_page(
    terminal: &Terminal,
    offset: usize,
    anchor: Option<HistorySource>,
    shift: i32,
) -> Result<
    (
        std::sync::Arc<jterm_engine::history_reflow::HistoryLayout>,
        usize,
        HistoryPage,
    ),
    String,
> {
    if offset > MAX_PROJECTED_HISTORY_ROWS
        || shift.unsigned_abs() as usize > MAX_PROJECTED_HISTORY_ROWS
        || (anchor.is_none() && shift != 0)
    {
        return Err("history page out of bounds".into());
    }
    let layout = terminal.history_layout()?;
    let start = anchor
        .map(|source| {
            let anchor = HistoryAnchor {
                row: source.row.get() - 1,
                column: source.column,
            };
            if !terminal.retains_history_anchor(anchor) {
                return Err("History source expired. Refresh to view retained rows.");
            }
            Ok(layout.row_at(anchor).saturating_add_signed(shift as isize))
        })
        .transpose()?;
    let mut limit = HISTORY_PAGE_ROWS;
    loop {
        let latest = layout.len().saturating_sub(limit);
        let first = start.map_or_else(|| latest.saturating_sub(offset), |start| start.min(latest));
        let page = terminal.projected_history_page(&layout, first, limit)?;
        if page.text.len() <= MAX_HISTORY_SNAPSHOT_BYTES {
            return Ok((layout, first, page));
        }
        if limit <= MIN_HISTORY_PAGE_ROWS {
            return Err("history snapshot exceeds its byte limit".into());
        }
        limit /= 2;
    }
}

fn bounded_history_page(terminal: &Terminal, offset: usize) -> Result<HistoryPage, String> {
    if offset > MAX_HISTORY_OFFSET {
        return Err("history page out of bounds".into());
    }
    let rows = terminal.history_rows();
    let mut limit = HISTORY_PAGE_ROWS;
    loop {
        let oldest_offset = rows.saturating_sub(limit);
        let page = terminal.history_page(offset.min(oldest_offset), limit)?;
        if page.text.len() <= MAX_HISTORY_SNAPSHOT_BYTES {
            return Ok(page);
        }
        if limit <= MIN_HISTORY_PAGE_ROWS {
            return Err("history snapshot exceeds its byte limit".into());
        }
        limit /= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projected_history_moves_raster_coordinates_and_rejects_evicted_anchors() {
        let mut terminal = Terminal::new(4, 10).unwrap();
        terminal.enable_inline_graphics((12, 24), |_, _, _| None);
        terminal
            .feed(b"abcdefghijxy\x1b_Ga=T,i=51,f=24,s=1,v=1,c=2,r=1,C=1;/wAA\x1b\\\x1b[4S")
            .unwrap();
        let (layout, first, before) = projected_history_page(&terminal, 0, None, 0).unwrap();
        let origin = layout.anchor_at(first).unwrap();
        let anchor = HistorySource {
            row: crate::protocol_v4::Counter::new(origin.row + 1).unwrap(),
            column: origin.column,
        };
        let (start_col, image, _) = before.inline_images(1).next().unwrap();
        assert_eq!(start_col, 2);
        let id = image.id();
        terminal.resize(4, 8).unwrap();
        let (_, _, after) = projected_history_page(&terminal, 0, Some(anchor), 0).unwrap();
        let (reflowed_col, image, _) = after.inline_images(1).next().unwrap();
        assert_eq!(reflowed_col, 4);
        assert_eq!(image.id(), id);
        assert_eq!(image.raster().unwrap().rgba(), &[255, 0, 0, 255]);
        assert_eq!(before.text, after.text);
        terminal.feed(b"\x1b[3J").unwrap();
        assert_eq!(
            projected_history_page(&terminal, 0, Some(anchor), 0)
                .err()
                .as_deref(),
            Some("History source expired. Refresh to view retained rows.")
        );
    }

    #[test]
    fn dense_unicode_history_respects_the_wire_budget_and_reaches_the_oldest_row() {
        let mut terminal = Terminal::new(2, 256).unwrap();
        terminal.enable_grapheme_clusters();
        let cluster = "e\u{301}\u{302}\u{303}\u{304}\u{305}";
        let body = cluster.repeat(248);
        for row in 0..170 {
            terminal
                .feed(format!("{row:03} {body}\r\n").as_bytes())
                .unwrap();
        }
        let newest = bounded_history_page(&terminal, 0).unwrap();
        assert_eq!(newest.row_count, 32);
        assert!(newest.text.len() <= MAX_HISTORY_SNAPSHOT_BYTES);
        assert!(newest.text.ends_with(&body));

        let oldest = bounded_history_page(&terminal, MAX_HISTORY_OFFSET).unwrap();
        assert_eq!(oldest.offset + oldest.row_count, oldest.total_rows);
        assert_eq!(
            oldest.text.lines().next(),
            Some(format!("000 {body}").as_str())
        );
    }

    #[test]
    fn prepared_editor_launch_has_its_own_epoch_and_uses_exact_argv() {
        use std::ffi::OsString;
        let cwd = std::env::temp_dir();
        let argv = [
            "/bin/sh",
            "-c",
            "printf '%s' \"$1\"",
            "sh",
            "AETHER PREPARED; EDITOR",
        ]
        .map(OsString::from);
        let mut bridge = LiveBridge::local_desktop();
        let epoch = bridge
            .open_prepared_editor(4, 32, &argv, &cwd, &[])
            .unwrap();
        let owner = bridge.attachment(epoch).unwrap();
        assert_eq!(owner.epoch, 1);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let active = bridge.pump_unpublished().unwrap();
            if !active {
                break;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let (_, frame) = bridge
            .observe_channel_frame(epoch, None, SnapshotFormat::Channel)
            .unwrap()
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(json["stamp"]["epoch"], "1");
        assert_eq!(json["stamp"]["session"], owner.session);
        let first_row: String = json["rows"][0]["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cell| cell["text"].as_str().unwrap())
            .collect();
        assert!(first_row.starts_with("AETHER PREPARED; EDITOR"));
        bridge.close(epoch).unwrap();
        assert!(bridge
            .open_prepared_editor(4, 32, &argv, &cwd, &[])
            .is_err());
    }

    #[test]
    fn busy_channel_encoder_defers_first_publication_but_drains_the_real_pty() {
        use std::{
            ffi::OsString,
            thread,
            time::{Duration, Instant},
        };

        let argv = ["/bin/sh", "-c", "printf 'OWNED WHILE CHANNEL IS BUSY'"].map(OsString::from);
        let mut bridge = LiveBridge::local_desktop();
        let epoch = bridge
            .open_prepared_editor(4, 40, &argv, &std::env::temp_dir(), &[])
            .unwrap();
        let owner = bridge.attachment(epoch).unwrap();
        let timeout = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(bridge
                .poll_channel_with_scratch(epoch, true, SnapshotFormat::Channel, None)
                .unwrap()
                .is_none());
            assert!(!bridge.has_unacknowledged_frame());
            if bridge.session.as_ref().unwrap().is_drained() {
                break;
            }
            assert!(Instant::now() < timeout);
            thread::sleep(Duration::from_millis(10));
        }
        let (_, frame) = bridge
            .poll_channel_frame(epoch, true, SnapshotFormat::Channel)
            .unwrap()
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(json["stamp"]["session"], owner.session);
        let line: String = json["rows"][0]["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cell| cell["text"].as_str().unwrap())
            .collect();
        assert!(line.starts_with("OWNED WHILE CHANNEL IS BUSY"));
        assert!(bridge.has_unacknowledged_frame());
        bridge.close(epoch).unwrap();
    }

    #[test]
    fn decoded_frame_pressure_drains_the_real_pty_and_recovers_the_same_attachment() {
        use jterm_engine::retained_bytes::RetainedBytesBudget;
        use std::{
            ffi::OsString,
            sync::Arc,
            thread,
            time::{Duration, Instant},
        };

        let capacity = 1024 * 1024;
        let budget = Arc::new(RetainedBytesBudget::new(capacity));
        let busy = budget.reserve(capacity).unwrap();
        let mut bridge = LiveBridge {
            frame_budget_override: Some(Arc::clone(&budget)),
            ..LiveBridge::local_desktop()
        };
        let argv = ["/bin/sh", "-c", "printf 'AFTER FRAME PRESSURE'"].map(OsString::from);
        let epoch = bridge
            .open_prepared_editor(4, 40, &argv, &std::env::temp_dir(), &[])
            .unwrap();
        let owner = bridge.attachment(epoch).unwrap();
        assert!(bridge.resume(Some(&owner)).is_err());
        assert_eq!(bridge.attachment(epoch).unwrap(), owner);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(bridge
                .poll_channel_frame(epoch, true, SnapshotFormat::Channel)
                .unwrap()
                .is_none());
            assert!(!bridge.has_unacknowledged_frame());
            if bridge.status(epoch).unwrap().drained {
                break;
            }
            assert!(Instant::now() < deadline, "PTY did not drain");
            thread::sleep(Duration::from_millis(10));
        }
        drop(busy);
        let (revision, frame) = bridge
            .poll_channel_frame(epoch, true, SnapshotFormat::Channel)
            .unwrap()
            .unwrap();
        let frame: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(frame["stamp"]["session"], owner.session);
        let line: String = frame["rows"][0]["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cell| cell["text"].as_str().unwrap())
            .collect();
        assert!(line.starts_with("AFTER FRAME PRESSURE"));
        let published_bytes = budget.usage().retained_bytes;
        assert!(published_bytes > 0);
        bridge
            .acknowledge_publication(epoch, revision.get())
            .unwrap();
        assert_eq!(bridge.attachment(epoch).unwrap(), owner);
        assert_eq!(budget.usage().retained_bytes, published_bytes);
        bridge.close(epoch).unwrap();
        assert_eq!(budget.usage().retained_bytes, 0);
    }

    #[test]
    fn semantic_stream_reconnects_to_the_same_pty_after_lost_delta_and_ack() {
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(16, 80).unwrap();
        let original = bridge.attachment(epoch).unwrap();
        let Some(Update::Snapshot { frame }) = bridge.poll_update(epoch).unwrap() else {
            panic!("missing initial snapshot");
        };
        bridge.acknowledge(epoch, frame.revision).unwrap();
        write_owned_line(
            &mut bridge,
            epoch,
            b"TOKEN=STILL_RUNNING; stty -echo; printf '\\033[10;1H%s\\r\\n' INITIAL_DELTA\n",
        );

        let mut accepted = frame.revision;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let unacknowledged = loop {
            assert!(
                std::time::Instant::now() < deadline,
                "visible delta missing"
            );
            match bridge.poll_update(epoch).unwrap() {
                Some(Update::Delta { frame }) => {
                    assert_eq!(frame.base_revision, accepted);
                    assert_eq!(frame.session, original.session);
                    assert!(frame.rows.len() < 16);
                    if frame.rows.iter().any(|row| {
                        row.cells
                            .iter()
                            .map(|cell| cell.text.as_str())
                            .collect::<String>()
                            .trim()
                            == "INITIAL_DELTA"
                    }) {
                        let revision = frame.revision;
                        let delta_len = serde_json::to_vec(&Update::Delta { frame }).unwrap().len();
                        let current = bridge.flight.as_ref().unwrap();
                        let snapshot_len = serde_json::to_vec(&Update::Snapshot {
                            frame: from_engine(current, &original.session, epoch).unwrap(),
                        })
                        .unwrap()
                        .len();
                        assert!(delta_len < snapshot_len);
                        println!(
                            "{{\"delta_bytes\":{delta_len},\"snapshot_bytes\":{snapshot_len}}}"
                        );
                        break revision;
                    }
                    accepted = frame.revision;
                    bridge.acknowledge(epoch, accepted).unwrap();
                }
                Some(Update::Snapshot { frame }) => {
                    accepted = frame.revision;
                    bridge.acknowledge(epoch, accepted).unwrap();
                }
                None => std::thread::sleep(std::time::Duration::from_millis(2)),
            }
        };
        assert!(bridge.poll_update(epoch).unwrap().is_none());

        let resumed = bridge.resume(Some(&original)).unwrap().unwrap();
        assert_eq!(resumed.owner.session, original.session);
        assert_eq!(resumed.owner.epoch, original.epoch);
        assert_ne!(resumed.owner.generation, original.generation);
        assert!(resumed.frame.revision >= unacknowledged);
        let retry = bridge.resume(Some(&original)).unwrap().unwrap();
        assert_eq!(retry.owner, resumed.owner);
        assert_eq!(retry.frame.revision, resumed.frame.revision);
        assert!(bridge.validate_attachment(&original).is_err());
        assert!(bridge.validate_input(&original).is_err());
        bridge.acknowledge(epoch, resumed.frame.revision).unwrap();
        assert!(bridge.resume(Some(&original)).is_err());

        assert_eq!(bridge.validate_input(&resumed.owner).unwrap(), epoch);
        write_owned_line(
            &mut bridge,
            epoch,
            b"printf '\\033[12;1H%s\\r\\n' \"$TOKEN\"\n",
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "shell variable did not survive"
            );
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                bridge.acknowledge(epoch, frame.revision).unwrap();
                if frame.rows.iter().any(|row| {
                    row.iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                        .trim()
                        == "STILL_RUNNING"
                }) {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        bridge.resize(epoch, 12, 72).unwrap();
        let Some(Update::Snapshot { frame }) = bridge.poll_update(epoch).unwrap() else {
            panic!("layout change must send a snapshot");
        };
        assert_eq!((frame.rows.len(), frame.cols), (12, 72));
        bridge.detach(&resumed.owner).unwrap();
        assert!(bridge.validate_attachment(&resumed.owner).is_err());
        let view_only = bridge.resume(None).unwrap().unwrap();
        assert_eq!(view_only.owner.session, original.session);
        assert!(bridge.validate_input(&view_only.owner).is_err());
        assert_eq!(bridge.validate_attachment(&view_only.owner).unwrap(), epoch);
        bridge.close(epoch).unwrap();
    }

    fn write_owned_line(bridge: &mut LiveBridge, epoch: u32, line: &[u8]) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut remaining = line;
        while !remaining.is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "test PTY write stalled"
            );
            let written = bridge.write(epoch, remaining).unwrap();
            remaining = &remaining[written..];
            if written == 0 {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
    #[test]
    fn real_pty_completed_cycle_publishes_before_next_hold_timeout() {
        use std::time::{Duration, Instant};
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(4, 80).unwrap();
        let first = bridge.poll(epoch).unwrap().unwrap();
        bridge.acknowledge(epoch, first.revision).unwrap();
        let command=b"stty -echo; printf '\\033[2J\\033[H\\033[?2026h\\101\\033[?2026l\\033[?2026h\\102'; read release; exit\n";
        write_owned_line(&mut bridge, epoch, command);
        let deadline = Instant::now() + Duration::from_secs(3);
        let complete = loop {
            assert!(Instant::now() < deadline);
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                if bridge
                    .session
                    .as_ref()
                    .unwrap()
                    .terminal()
                    .publication_held()
                {
                    break frame;
                }
                bridge.acknowledge(epoch, frame.revision).unwrap();
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(complete.rows[0][0].text, "A");
        assert_eq!(complete.rows[0][1].text, " ");
        assert!(bridge
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .visible_text()
            .contains("AB"));
        assert!(bridge.poll(epoch).unwrap().is_none());
        let recovered = bridge.recover_frame(epoch).unwrap();
        assert_eq!(complete.revision, recovered.revision);
        bridge.acknowledge(epoch, recovered.revision).unwrap();
        let live = loop {
            assert!(Instant::now() < deadline);
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                break frame;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(live.revision > complete.revision);
        assert_eq!(live.rows[0][1].text, "B");
        assert!(bridge.acknowledge(epoch, complete.revision).is_err());
        bridge.acknowledge(epoch, live.revision).unwrap();
        bridge.close(epoch).unwrap();
    }
    #[test]
    fn real_pty_sync_holds_publication_but_drains_and_recovers_stable() {
        use std::time::{Duration, Instant};
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(8, 64).unwrap();
        let first = bridge.poll(epoch).unwrap().unwrap();
        bridge.acknowledge(epoch, first.revision).unwrap();
        let command =
            b"stty -echo; printf '\\033[?2026h\\123\\131\\116\\103\\137\\120\\101\\122\\124'; read release; printf '\\123\\131\\116\\103\\137\\105\\116\\104\\033[?2026l'; exit\n";
        write_owned_line(&mut bridge, epoch, command);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !bridge
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .publication_held()
        {
            assert!(Instant::now() < deadline);
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                bridge.acknowledge(epoch, frame.revision).unwrap();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(bridge
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .visible_text()
            .contains("SYNC_PART"));
        let stable = bridge.stable.as_ref().unwrap().stamp().revision as u32;
        assert!(bridge.poll(epoch).unwrap().is_none());
        let recovered = bridge.recover_frame(epoch).unwrap();
        assert_eq!(recovered.revision, stable);
        assert!(!recovered.rows.iter().any(|r| r
            .iter()
            .map(|c| c.text.as_str())
            .collect::<String>()
            .contains("SYNC_PART")));
        bridge.acknowledge(epoch, recovered.revision).unwrap();
        assert!(bridge.recover_frame(epoch + 1).is_err());
        write_owned_line(&mut bridge, epoch, b"done\n");
        let mut released = false;
        while !bridge.status(epoch).unwrap().drained {
            assert!(Instant::now() < deadline);
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                released |= frame.rows.iter().any(|r| {
                    r.iter()
                        .map(|c| c.text.as_str())
                        .collect::<String>()
                        .contains("SYNC_PARTSYNC_END")
                });
                bridge.acknowledge(epoch, frame.revision).unwrap();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(released);
        bridge.close(epoch).unwrap();
    }
    #[test]
    fn real_pty_pressure_lost_ack_and_full_recovery() {
        use std::time::{Duration, Instant};
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(8, 64).unwrap();
        let first = bridge.poll(epoch).unwrap().unwrap();
        let command = b"i=0; while [ $i -lt 1400 ]; do printf 'PRESSURE_%s\\n' \"$i\"; i=$((i+1)); done; printf 'FINAL_SENTINEL\\n'; exit\n";
        let mut at = 0;
        let deadline = Instant::now() + Duration::from_secs(15);
        while !bridge.status(epoch).unwrap().drained {
            assert!(Instant::now() < deadline, "PTY did not drain");
            if at < command.len() {
                at += bridge.write(epoch, &command[at..]).unwrap();
            }
            // Withheld ACK never stops bounded PTY draining or allocates queued frames.
            assert!(bridge.poll(epoch).unwrap().is_none());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(at, command.len());
        let recovered = bridge.recover_frame(epoch).unwrap();
        assert!(recovered.revision > first.revision);
        assert!(recovered.rows.iter().any(|row| row
            .iter()
            .map(|c| c.text.as_str())
            .collect::<String>()
            .contains("FINAL_SENTINEL")));
        assert_eq!(bridge.history(epoch, 0).unwrap().total_rows, 1024);
        assert!(bridge.acknowledge(epoch, first.revision).is_err());
        bridge.acknowledge(epoch, recovered.revision).unwrap();
        bridge.acknowledge(epoch, recovered.revision).unwrap();
        assert!(bridge.poll(epoch).unwrap().is_none());
        assert!(bridge.recover_frame(epoch + 1).is_err());
        bridge.close(epoch).unwrap();
    }
    #[test]
    fn real_pty_begin_before_first_publication_recovers_initial_frame() {
        use std::time::{Duration, Instant};
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(4, 40).unwrap();
        let cmd = b"printf '\\033[?2026h'; read release; exit\n";
        write_owned_line(&mut bridge, epoch, cmd);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !bridge
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .publication_held()
        {
            assert!(Instant::now() < deadline);
            bridge.session.as_mut().unwrap().poll().unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(bridge.poll(epoch).unwrap().is_none());
        let frame = bridge.recover_frame(epoch).unwrap();
        assert_eq!(frame.revision, 1);
        bridge.acknowledge(epoch, frame.revision).unwrap();
        bridge.close(epoch).unwrap();
    }
    #[test]
    fn real_pty_final_drain_releases_unclosed_hold() {
        use std::time::{Duration, Instant};
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(4, 40).unwrap();
        let command = b"printf '\\033[?2026h\\104\\122\\101\\111\\116'; exit\n";
        write_owned_line(&mut bridge, epoch, command);
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut drain_published = false;
        while !bridge.status(epoch).unwrap().drained {
            assert!(Instant::now() < deadline);
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                let has_final = frame.rows.iter().any(|r| {
                    r.iter()
                        .map(|c| c.text.as_str())
                        .collect::<String>()
                        .contains("DRAIN")
                });
                if has_final {
                    assert!(bridge.status(epoch).unwrap().drained);
                    drain_published = true;
                }
                bridge.acknowledge(epoch, frame.revision).unwrap();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            drain_published,
            "the final poll must paint before the client stops polling"
        );
        bridge.close(epoch).unwrap();
    }
    #[test]
    fn real_pty_missing_end_releases_while_child_still_running() {
        use std::time::{Duration, Instant};
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(4, 48).unwrap();
        let start = Instant::now();
        write_owned_line(
            &mut bridge,
            epoch,
            b"printf '\\033[?2026h\\124\\111\\115\\105\\117\\125\\124'; read release; exit\n",
        );
        loop {
            assert!(start.elapsed() < Duration::from_secs(3));
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                bridge.acknowledge(epoch, frame.revision).unwrap();
                if frame.rows.iter().any(|r| {
                    r.iter()
                        .map(|c| c.text.as_str())
                        .collect::<String>()
                        .contains("TIMEOUT")
                }) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(start.elapsed() >= Duration::from_millis(250));
        assert!(!bridge.status(epoch).unwrap().exited);
        bridge.close(epoch).unwrap();
    }
    #[test]
    fn queue_admission_is_bounded_atomic_and_epoch_scoped() {
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(8, 64).unwrap();
        bridge.enqueue(epoch, vec![b'a'; 16384]).unwrap();
        assert!(bridge.key(epoch, "a", false, false, false).is_err());
        assert!(!bridge.session.as_ref().unwrap().input_idle());
        assert!(bridge.paste(epoch + 1, "hello").is_err());
        bridge.detach(&bridge.attachment(epoch).unwrap()).unwrap();
        assert!(bridge.session.as_ref().unwrap().input_idle());
        assert!(bridge.status(epoch).unwrap().write_error.is_none());
        bridge.close(epoch).unwrap();
        let next = bridge.open(8, 64).unwrap();
        assert!(bridge.session.as_ref().unwrap().input_idle());
        assert!(bridge.key(epoch, "a", false, false, false).is_err());
        bridge.key(next, "ArrowUp", false, false, false).unwrap();
        assert!(!bridge.session.as_ref().unwrap().input_idle());
        bridge.close(next).unwrap();
    }
    #[test]
    fn lifecycle_epoch_and_one_flight() {
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(8, 64).unwrap();
        assert!(bridge.open(8, 64).is_err());
        assert!(bridge.write(epoch + 1, b"x").is_err());
        assert!(bridge.status(epoch + 1).is_err());
        assert!(bridge.history(epoch + 1, 0).is_err());
        assert!(bridge.history(epoch, usize::MAX).is_err());
        assert_eq!(bridge.history(epoch, 0).unwrap().epoch, epoch);
        assert_eq!(bridge.status(epoch).unwrap().unsupported, "0");
        let frame = bridge.poll(epoch).unwrap().unwrap();
        assert_eq!(frame.session, bridge.attachment(epoch).unwrap().session);
        assert!(bridge.poll(epoch).unwrap().is_none());
        bridge.history(epoch, 0).unwrap();
        assert!(bridge.poll(epoch).unwrap().is_none());
        assert!(bridge.acknowledge(epoch, frame.revision + 1).is_err());
        bridge.acknowledge(epoch, frame.revision).unwrap();
        bridge.close(epoch).unwrap();
        assert!(bridge.write(epoch, b"x").is_err());
        let next = bridge.open(8, 64).unwrap();
        assert!(next > epoch);
        assert!(bridge.poll(epoch).is_err());
        bridge.close(next).unwrap();
    }
}
