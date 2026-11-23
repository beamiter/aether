use jterm_core::chat_mode::{resolve_surface, ChatSessionState, ChatSurface};
use serde::Serialize;
use tauri_terminal_frame_bridge::{
    live::{LiveBridge, SessionStatus},
    protocol_v4::Counter,
};

#[derive(Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LocalShellStatus {
    #[serde(flatten)]
    shell: SessionStatus,
    chat: TerminalChatStatus,
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
            chat: TerminalChatStatus {
                surface,
                prompt,
                newest_completed: activity.newest_completed,
            },
        })
    }
}
