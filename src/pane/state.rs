use crate::terminal::TerminalId;

/// Viewport state for a pane.
///
/// Terminal identity, cwd, labels, and agent metadata live in TerminalState.
pub struct PaneState {
    pub attached_terminal_id: TerminalId,
    /// Whether the user has seen this pane since its last state change to Idle.
    /// False = "Done" (agent finished while user was in another workspace).
    pub seen: bool,
    /// Whether unmodified right-click gestures should be forwarded to the pane application.
    pub right_click_passthrough: bool,
    /// The last fresh agent session (startup, clear, or new) reported here.
    pub fresh_session: Option<FreshSession>,
}

/// A fresh agent session and when it started, relative to other panes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshSession {
    /// Server-wide counter value; larger started later.
    pub order: u64,
    /// None until an agent that starts empty, like OpenCode on its home
    /// screen, creates the session.
    pub session_ref: Option<crate::agent_resume::AgentSessionRef>,
    /// The agent has worked since the session started, so it is no longer empty.
    pub used: bool,
}

impl PaneState {
    pub fn new(attached_terminal_id: TerminalId) -> Self {
        Self {
            attached_terminal_id,
            seen: true,
            right_click_passthrough: false,
            fresh_session: None,
        }
    }

    /// Whether the agent is on a fresh session it has not worked in yet.
    pub fn session_empty(&self) -> bool {
        self.fresh_session.as_ref().is_some_and(|fresh| !fresh.used)
    }
}
