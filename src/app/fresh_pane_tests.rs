//! Moving panes whose agent starts a fresh session, and sorting panes by age.

use std::time::Instant;

use ratatui::layout::Direction;

use crate::agent_resume::AgentSessionRef;
use crate::app::state::AppState;
use crate::detect::Agent;
use crate::events::AppEvent;
use crate::layout::PaneId;

/// A tab with three panes in layout order, each running Claude.
fn three_claude_panes() -> (AppState, [PaneId; 3]) {
    let mut app = AppState::test_with_adversarial_identity_state();
    let first = app.workspaces[0].tabs[0].root_pane;
    let second = app.workspaces[0].test_split(Direction::Horizontal);
    let third = app.workspaces[0].test_split(Direction::Horizontal);
    app.ensure_test_terminals();
    let panes = [first, second, third];
    assert_eq!(app.workspaces[0].tabs[0].layout.pane_ids(), panes);
    for pane_id in panes {
        app.handle_app_event(AppEvent::AgentProcessDetected {
            pane_id,
            agent: Agent::Claude,
            observed_at: Instant::now(),
        });
    }
    (app, panes)
}

fn session(id: u32) -> AgentSessionRef {
    AgentSessionRef::id(format!("2f0c0c2e-1647-4a4b-9a3e-{id:012}")).expect("claude session id")
}

fn report(app: &mut AppState, pane_id: PaneId, id: u32, seq: u64, source: &str) {
    app.handle_app_event(AppEvent::AgentSessionReported {
        pane_id,
        source: "flockr:claude".into(),
        agent_label: "claude".into(),
        seq: Some(seq),
        session_ref: Some(session(id)),
        session_start_source: Some(source.into()),
    });
}

fn order(app: &AppState) -> Vec<PaneId> {
    app.workspaces[0].tabs[0].layout.pane_ids()
}

#[test]
fn fresh_session_moves_ahead_of_older_empty_sessions() {
    let (mut app, [first, second, third]) = three_claude_panes();
    app.keep_agents_in_use_first = true;
    let focused = app.workspaces[0].tabs[0].layout.focused();

    report(&mut app, first, 1, 1, "clear");

    assert_eq!(order(&app), vec![second, third, first]);
    assert_eq!(app.workspaces[0].tabs[0].layout.focused(), focused);

    // The newest empty session goes first among the empty ones, whether
    // the agent was launched or cleared, and the first prompt leaves it
    // there.
    for (source, seq) in [("startup", 1), ("clear", 2)] {
        app.workspaces[0].tabs[0]
            .layout
            .reorder_panes(&[third, first, second]);
        report(&mut app, second, 10 + seq as u32, seq, source);
        assert_eq!(order(&app), vec![third, second, first], "{source}");
    }
    working(&mut app, second, Agent::Claude);
    assert_eq!(order(&app), vec![third, second, first]);
    app.assert_invariants_for_test();
}

#[test]
fn fresh_session_leaves_layout_alone_when_disabled() {
    let (mut app, panes) = three_claude_panes();

    report(&mut app, panes[0], 1, 1, "clear");

    assert_eq!(order(&app), panes);
    assert!(app.workspaces[0].tabs[0].panes[&panes[0]]
        .fresh_session
        .is_some());
}

#[test]
fn resumed_or_repeated_sessions_do_not_move_panes() {
    let (mut app, [first, second, third]) = three_claude_panes();
    app.keep_agents_in_use_first = true;

    report(&mut app, first, 1, 1, "resume");
    assert_eq!(order(&app), vec![first, second, third]);

    report(&mut app, second, 2, 1, "startup");
    assert_eq!(order(&app), vec![first, third, second]);

    // The same session reported again, as a duplicate hook would.
    app.workspaces[0].tabs[0]
        .layout
        .reorder_panes(&[second, first, third]);
    report(&mut app, second, 2, 2, "startup");
    assert_eq!(order(&app), vec![second, first, third]);
}

#[test]
fn sort_by_age_puts_newest_fresh_session_last() {
    let (mut app, [first, second, third]) = three_claude_panes();

    report(&mut app, second, 2, 1, "startup");
    report(&mut app, first, 1, 1, "clear");
    assert_eq!(order(&app), vec![first, second, third]);

    assert!(app.sort_tab_panes_by_age(0, 0));

    // third never reported a fresh session, so it counts as oldest.
    assert_eq!(order(&app), vec![third, second, first]);
    assert!(!app.sort_tab_panes_by_age(0, 0));
}

fn opencode_report(app: &mut AppState, pane_id: PaneId, session: Option<&str>, source: &str) {
    app.handle_app_event(AppEvent::AgentSessionReported {
        pane_id,
        source: "flockr:opencode".into(),
        agent_label: "opencode".into(),
        seq: None,
        session_ref: session.map(|id| AgentSessionRef::id(id).expect("opencode session id")),
        session_start_source: Some(source.into()),
    });
}

#[test]
fn opencode_home_screen_counts_as_fresh_until_its_session_is_created() {
    let mut app = AppState::test_with_adversarial_identity_state();
    let first = app.workspaces[0].tabs[0].root_pane;
    let second = app.workspaces[0].test_split(Direction::Horizontal);
    app.ensure_test_terminals();
    app.keep_agents_in_use_first = true;
    app.handle_app_event(AppEvent::AgentProcessDetected {
        pane_id: first,
        agent: Agent::OpenCode,
        observed_at: Instant::now(),
    });
    app.handle_app_event(AppEvent::AgentProcessDetected {
        pane_id: second,
        agent: Agent::Claude,
        observed_at: Instant::now(),
    });

    // Launch lands on the home screen.
    opencode_report(&mut app, first, None, "new");
    assert_eq!(order(&app), vec![second, first]);

    // The plugin repeats the home report; the first prompt creates a session.
    app.workspaces[0].tabs[0]
        .layout
        .reorder_panes(&[first, second]);
    opencode_report(&mut app, first, None, "new");
    opencode_report(&mut app, first, Some("ses_a"), "select");
    assert_eq!(order(&app), vec![first, second]);

    // /new returns to the home screen.
    opencode_report(&mut app, first, None, "new");
    assert_eq!(order(&app), vec![second, first]);
}

#[test]
fn session_less_fresh_report_needs_that_agent_in_the_pane() {
    let (mut app, [first, second, third]) = three_claude_panes();
    app.keep_agents_in_use_first = true;

    opencode_report(&mut app, first, None, "new");

    assert_eq!(order(&app), vec![first, second, third]);
}

#[test]
fn fresh_session_is_empty_until_the_agent_works() {
    let (mut app, [first, ..]) = three_claude_panes();
    let empty = |app: &AppState| app.workspaces[0].tabs[0].panes[&first].session_empty();
    assert!(!empty(&app));

    report(&mut app, first, 1, 1, "clear");
    assert!(empty(&app));

    app.handle_app_event(AppEvent::StateChanged {
        pane_id: first,
        agent: Some(Agent::Claude),
        state: crate::detect::AgentState::Working,
        visible_blocker: false,
        visible_working: true,
        process_exited: false,
        observed_at: Instant::now(),
    });
    assert!(!empty(&app));

    report(&mut app, first, 2, 2, "clear");
    assert!(empty(&app));
}

fn working(app: &mut AppState, pane_id: PaneId, agent: Agent) {
    app.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(agent),
        state: crate::detect::AgentState::Working,
        visible_blocker: false,
        visible_working: true,
        process_exited: false,
        observed_at: Instant::now(),
    });
}

#[test]
fn first_prompt_moves_pane_after_panes_in_use() {
    let (mut app, [first, second, third]) = three_claude_panes();
    app.keep_agents_in_use_first = true;
    report(&mut app, first, 1, 1, "clear");
    report(&mut app, second, 2, 1, "clear");
    assert_eq!(order(&app), vec![third, second, first]);

    // first takes a prompt: it goes after third, ahead of the still-empty
    // second.
    working(&mut app, first, Agent::Claude);
    assert_eq!(order(&app), vec![third, first, second]);

    // Working again does not move it.
    app.workspaces[0].tabs[0]
        .layout
        .reorder_panes(&[first, third, second]);
    working(&mut app, first, Agent::Claude);
    assert_eq!(order(&app), vec![first, third, second]);
    app.assert_invariants_for_test();
}

#[test]
fn agent_panes_move_and_sort_only_within_their_run() {
    let (mut app, [first, second, third]) = three_claude_panes();
    let shell = app.workspaces[0].test_split(Direction::Horizontal);
    app.ensure_test_terminals();
    app.keep_agents_in_use_first = true;
    app.workspaces[0].tabs[0]
        .layout
        .reorder_panes(&[first, shell, second, third]);

    // second moves to the end of its run, past third but not the shell.
    report(&mut app, second, 2, 1, "clear");
    assert_eq!(order(&app), vec![first, shell, third, second]);

    // first is alone in its run, so it stays ahead of the shell.
    report(&mut app, first, 1, 1, "clear");
    working(&mut app, first, Agent::Claude);
    assert_eq!(order(&app), vec![first, shell, third, second]);

    app.workspaces[0].tabs[0]
        .layout
        .reorder_panes(&[second, third, shell, first]);
    assert!(app.sort_tab_panes_by_age(0, 0));
    assert_eq!(order(&app), vec![third, second, shell, first]);
    app.assert_invariants_for_test();
}

#[test]
fn codex_session_start_on_first_prompt_moves_pane_after_panes_in_use() {
    let (mut app, [first, second, third]) = three_claude_panes();
    app.keep_agents_in_use_first = true;
    report(&mut app, second, 2, 1, "clear");
    assert_eq!(order(&app), vec![first, third, second]);
    app.handle_app_event(AppEvent::AgentProcessDetected {
        pane_id: first,
        agent: Agent::Codex,
        observed_at: Instant::now(),
    });

    // Codex reports its session start with the first prompt.
    app.handle_app_event(AppEvent::AgentSessionReported {
        pane_id: first,
        source: "flockr:codex".into(),
        agent_label: "codex".into(),
        seq: Some(1),
        session_ref: Some(AgentSessionRef::id("codex-session").expect("codex session id")),
        session_start_source: Some("startup".into()),
    });

    assert_eq!(order(&app), vec![third, first, second]);
    assert!(!app.workspaces[0].tabs[0].panes[&first].session_empty());
}

#[cfg(unix)]
#[test]
fn handoff_keeps_fresh_sessions() {
    let (mut app, [first, second, _]) = three_claude_panes();
    report(&mut app, first, 1, 1, "clear");
    report(&mut app, second, 2, 1, "clear");
    working(&mut app, second, Agent::Claude);
    let snapshot = crate::persist::capture(
        &app.workspaces,
        &app.terminals,
        &crate::terminal::TerminalRuntimeRegistry::default(),
        Some(0),
        0,
    );
    for pane in app.workspaces[0].tabs[0].panes.values_mut() {
        pane.fresh_session = None;
    }

    let max_order = crate::persist::restore_handoff_fresh_sessions(&snapshot, &mut app.workspaces);

    let panes = &app.workspaces[0].tabs[0].panes;
    assert!(panes[&first].session_empty());
    assert!(!panes[&second].session_empty());
    assert!(panes[&second].fresh_session.is_some());
    assert_eq!(max_order, app.fresh_session_order);
}
