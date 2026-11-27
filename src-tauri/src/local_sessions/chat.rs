use jterm_core::chat_mode::{resolve_surface, ChatSessionState, ChatSurface};
use serde::Serialize;
use tauri_terminal_frame_bridge::{
    live::{AlternateScrollView, LiveBridge, MouseView, SessionStatus},
    protocol_v4::Counter,
};

#[derive(Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LocalShellStatus {
    #[serde(flatten)]
    shell: SessionStatus,
    chat: TerminalChatStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    mouse: Option<MouseView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    alternate_scroll: Option<AlternateScrollView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    history_oldest_row: Option<Counter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pane_history_id: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
struct TerminalChatStatus {
    surface: &'static str,
    prompt: Option<Counter>,
    newest_completed: Option<Counter>,
}

impl LocalShellStatus {
    pub(super) fn drained(&self) -> bool {
        self.shell.drained
    }

    pub(crate) fn read(bridge: &LiveBridge, epoch: u32) -> Result<Self, String> {
        let shell = bridge.status(epoch)?;
        let activity = bridge.command_activity(epoch)?;
        let surface = resolve_surface(ChatSessionState {
            shell_integration: shell.command_blocks_enabled,
            prompt_ready: activity.prompt.is_some() && !shell.exited,
            command_running: activity.running && !shell.exited,
            alternate_screen: activity.alternate_screen,
        });
        let prompt = activity
            .prompt
            .filter(|_| surface.allows_submit_affordance());
        let surface = match surface {
            ChatSurface::Prompt => "prompt",
            ChatSurface::Running => "running",
            ChatSurface::Passthrough => "passthrough",
            ChatSurface::Fullscreen => "fullscreen",
        };
        Ok(Self {
            shell,
            mouse: bridge.mouse_view(epoch)?,
            alternate_scroll: bridge.alternate_scroll_view(epoch)?,
            history_oldest_row: bridge.history_oldest_row(epoch)?,
            pane_history_id: bridge.completed_pane_id().map(str::to_owned),
            chat: TerminalChatStatus {
                surface,
                prompt,
                newest_completed: bridge.completed_pane_cursor(epoch)?,
            },
        })
    }
}
