use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::api::schema::AgentStatus;
use crate::config::SeenScopeConfig;
use crate::protocol::{ClientShellAgent, ClientShellSnapshot, PaneSurfaceFrame};

#[derive(Clone, Debug, Default)]
pub(super) struct EndpointAgentPresentation {
    boot_id: Option<String>,
    acknowledged: HashMap<String, u64>,
    /// Completions the server reports read, from any client or device.
    server_reads: HashMap<String, u64>,
    /// Completions this client acknowledged that the server has not been told
    /// about yet.
    unsent_reads: BTreeMap<String, u64>,
    completed: HashMap<String, u64>,
    working: HashSet<String>,
    /// When each agent pane on screen started being shown, for `seen_delay_ms`.
    viewing_since: HashMap<String, Instant>,
    /// Earliest time a shown completion becomes due for acknowledgement.
    seen_deadline: Option<Instant>,
    /// A pane marked unread while shown; not acknowledged until it leaves.
    held: Option<String>,
    pending_completions: Option<(
        Option<u64>,
        crate::protocol::endpoint::EndpointAgentCompletions,
    )>,
}

impl EndpointAgentPresentation {
    pub(super) fn receive_completions(
        &mut self,
        generation: Option<u64>,
        completions: crate::protocol::endpoint::EndpointAgentCompletions,
    ) {
        if self
            .pending_completions
            .as_ref()
            .is_some_and(|(current_generation, current)| {
                *current_generation == generation
                    && current.boot_id == completions.boot_id
                    && current.revision >= completions.revision
            })
        {
            return;
        }
        self.pending_completions = Some((generation, completions));
    }

    #[cfg(test)]
    pub(super) fn project_snapshot(&mut self, snapshot: &mut ClientShellSnapshot) {
        self.project_snapshot_for_generation(snapshot, None);
    }

    pub(super) fn project_snapshot_for_generation(
        &mut self,
        snapshot: &mut ClientShellSnapshot,
        generation: Option<u64>,
    ) {
        if self.boot_id.as_deref() != Some(snapshot.boot_id.as_str()) {
            self.boot_id = Some(snapshot.boot_id.clone());
            self.acknowledged.clear();
            self.server_reads.clear();
            self.unsent_reads.clear();
            self.completed.clear();
            self.working.clear();
            self.acknowledged.extend(
                snapshot
                    .agents
                    .iter()
                    .map(|agent| (agent.pane_id.clone(), agent.state_change_seq)),
            );
        }
        let pane_ids: HashSet<&str> = snapshot
            .agents
            .iter()
            .map(|agent| agent.pane_id.as_str())
            .collect();
        self.acknowledged
            .retain(|pane_id, _| pane_ids.contains(pane_id.as_str()));
        self.server_reads
            .retain(|pane_id, _| pane_ids.contains(pane_id.as_str()));
        self.unsent_reads
            .retain(|pane_id, _| pane_ids.contains(pane_id.as_str()));
        self.completed
            .retain(|pane_id, _| pane_ids.contains(pane_id.as_str()));
        self.working
            .retain(|pane_id| pane_ids.contains(pane_id.as_str()));
        let completions = self
            .pending_completions
            .take()
            .filter(|(received_generation, projection)| {
                *received_generation == generation
                    && projection.boot_id == snapshot.boot_id
                    && projection.revision == snapshot.revision
            })
            .map(|(_, projection)| {
                self.server_reads = projection.unread.map_or_else(HashMap::new, |unread| {
                    projection
                        .completions
                        .iter()
                        .filter(|(pane_id, _)| !unread.contains(*pane_id))
                        .map(|(pane_id, seq)| (pane_id.clone(), *seq))
                        .collect()
                });
                projection.completions
            });
        for agent in &mut snapshot.agents {
            match agent.agent_status {
                AgentStatus::Working => {
                    self.working.insert(agent.pane_id.clone());
                    self.completed.remove(&agent.pane_id);
                }
                AgentStatus::Blocked => {
                    self.completed.remove(&agent.pane_id);
                }
                AgentStatus::Idle | AgentStatus::Done => {
                    let observed_work = self.working.remove(&agent.pane_id);
                    let completed = completions.as_ref().map_or_else(
                        || {
                            observed_work
                                || self.completed.get(&agent.pane_id)
                                    == Some(&agent.state_change_seq)
                        },
                        |completions| {
                            completions.get(&agent.pane_id) == Some(&agent.state_change_seq)
                        },
                    );
                    if completed {
                        self.completed
                            .insert(agent.pane_id.clone(), agent.state_change_seq);
                    } else {
                        self.completed.remove(&agent.pane_id);
                    }
                }
                _ => {
                    self.working.remove(&agent.pane_id);
                    self.completed.remove(&agent.pane_id);
                }
            }
            agent.agent_status = self.projected_status(agent);
        }
        project_aggregate_status(snapshot);
    }

    pub(super) fn acknowledge_surface(
        &mut self,
        snapshot: &mut ClientShellSnapshot,
        surface: &PaneSurfaceFrame,
        outer_focused: Option<bool>,
        now: Instant,
        seen_delay: Duration,
        scope: SeenScopeConfig,
    ) -> bool {
        if outer_focused == Some(false) {
            self.viewing_since.clear();
            self.seen_deadline = None;
            return false;
        }
        if self.boot_id.as_deref() != Some(surface.boot_id.as_str())
            || snapshot.boot_id != surface.boot_id
            || snapshot.revision != surface.projection_revision
        {
            return false;
        }

        // With seen_scope "pane" only the focused pane counts as shown.
        let shown = |pane: &&crate::protocol::PaneSurfacePane| {
            scope == SeenScopeConfig::Tab || pane.focused
        };
        // A pane that leaves the screen starts its delay over next time it is shown.
        self.viewing_since.retain(|pane_id, _| {
            surface
                .panes
                .iter()
                .filter(shown)
                .any(|pane| &pane.pane_id == pane_id)
        });
        if self.held.as_ref().is_some_and(|held| {
            !surface
                .panes
                .iter()
                .filter(shown)
                .any(|pane| &pane.pane_id == held)
        }) {
            self.held = None;
        }
        self.seen_deadline = None;
        let mut changed = false;
        for pane in surface.panes.iter().filter(shown) {
            let Some(agent) = snapshot
                .agents
                .iter()
                .find(|agent| agent.pane_id == pane.pane_id)
            else {
                continue;
            };
            let since = *self
                .viewing_since
                .entry(agent.pane_id.clone())
                .or_insert(now);
            if self.held.as_ref() == Some(&agent.pane_id) {
                continue;
            }
            let acknowledged = self.acknowledged.entry(agent.pane_id.clone()).or_default();
            if *acknowledged >= agent.state_change_seq {
                continue;
            }
            let due = since + seen_delay;
            if now < due {
                self.seen_deadline =
                    Some(self.seen_deadline.map_or(due, |deadline| deadline.min(due)));
                continue;
            }
            *acknowledged = agent.state_change_seq;
            changed = true;
            if let Some(&completion) = self.completed.get(&agent.pane_id) {
                if completion <= agent.state_change_seq
                    && self
                        .server_reads
                        .get(&agent.pane_id)
                        .is_none_or(|read| *read < completion)
                {
                    self.unsent_reads.insert(agent.pane_id.clone(), completion);
                }
            }
        }
        if changed {
            for agent in &mut snapshot.agents {
                agent.agent_status = self.projected_status(agent);
            }
            project_aggregate_status(snapshot);
        }
        changed
    }

    /// Keeps `pane_id` from being acknowledged while it stays on screen, so
    /// marking the shown pane unread sticks until you leave it.
    pub(super) fn hold_unread(&mut self, pane_id: String) {
        self.held = Some(pane_id);
    }

    pub(super) fn seen_deadline(&self) -> Option<Instant> {
        self.seen_deadline
    }

    /// Completions this client acknowledged since the last call, as
    /// `(pane_id, completion_seq)`, for the server to mark read.
    pub(super) fn take_unsent_reads(&mut self) -> Vec<(String, u64)> {
        std::mem::take(&mut self.unsent_reads).into_iter().collect()
    }

    /// A completion is seen once this client acknowledged it or the server
    /// reports it read; a read from anywhere wins over a local hold.
    pub(super) fn seen(&self, agent: &ClientShellAgent) -> bool {
        self.completed.get(&agent.pane_id).is_none_or(|completion| {
            [&self.acknowledged, &self.server_reads]
                .iter()
                .any(|reads| reads.get(&agent.pane_id) >= Some(completion))
        })
    }

    fn projected_status(&self, agent: &ClientShellAgent) -> AgentStatus {
        match agent.agent_status {
            AgentStatus::Idle | AgentStatus::Done => {
                if self.seen(agent) {
                    AgentStatus::Idle
                } else {
                    AgentStatus::Done
                }
            }
            status => status,
        }
    }
}

fn project_aggregate_status(snapshot: &mut ClientShellSnapshot) {
    for tab in &mut snapshot.tabs {
        if let Some(status) = snapshot
            .agents
            .iter()
            .filter(|agent| agent.tab_id == tab.tab_id)
            .map(|agent| agent.agent_status)
            .max_by_key(|status| super::status_priority(*status))
        {
            tab.agent_status = status;
        }
    }
    for workspace in &mut snapshot.workspaces {
        if let Some(status) = snapshot
            .agents
            .iter()
            .filter(|agent| agent.workspace_id == workspace.workspace_id)
            .map(|agent| agent.agent_status)
            .max_by_key(|status| super::status_priority(*status))
        {
            workspace.agent_status = status;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        endpoint::EndpointAgentCompletions, FrameData, PaneSurfacePane, SurfaceRect,
    };

    fn agent(status: AgentStatus, sequence: u64) -> ClientShellAgent {
        ClientShellAgent {
            pane_id: "agent-pane".into(),
            workspace_id: "workspace".into(),
            tab_id: "tab".into(),
            name: None,
            display_agent: None,
            agent: None,
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: status,
            state_change_seq: sequence,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: true,
            branch: None,
            session_empty: false,
        }
    }

    fn snapshot(status: AgentStatus, sequence: u64, revision: u64) -> ClientShellSnapshot {
        let mut snapshot = crate::client::shell::tests::snapshot();
        snapshot.boot_id = "endpoint-boot".into();
        snapshot.revision = revision;
        snapshot.agents = vec![agent(status, sequence)];
        snapshot
    }

    fn surface(revision: u64) -> PaneSurfaceFrame {
        let rect = SurfaceRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        PaneSurfaceFrame {
            boot_id: "endpoint-boot".into(),
            projection_revision: revision,
            surface_revision: 1,
            frame: FrameData {
                cells: Vec::new(),
                width: 0,
                height: 0,
                cursor: None,
                hyperlinks: Vec::new(),
                graphics: Vec::new(),
            },
            panes: vec![PaneSurfacePane {
                pane_id: "agent-pane".into(),
                content_revision: 1,
                rect,
                inner_rect: rect,
                scrollbar_rect: None,
                scroll: None,
                focused: true,
                mouse_reporting: false,
                sgr_pixel_mouse: false,
                alternate_screen_active: false,
                pixel_width: 0,
                pixel_height: 0,
            }],
            splits: Vec::new(),
            popup: None,
            graphics: Default::default(),
        }
    }

    #[test]
    fn first_snapshot_establishes_an_idle_baseline_without_server_seen_authority() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut snapshot = snapshot(AgentStatus::Done, 4, 1);

        presentation.project_snapshot(&mut snapshot);

        assert_eq!(snapshot.agents[0].agent_status, AgentStatus::Idle);
    }

    fn assert_idle_sequence(states: &[(AgentStatus, u64)]) {
        let mut presentation = EndpointAgentPresentation::default();
        let mut projected = AgentStatus::Unknown;
        for (revision, &(status, seq)) in states.iter().enumerate() {
            let mut snapshot = snapshot(status, seq, revision as u64 + 1);
            presentation.project_snapshot(&mut snapshot);
            projected = snapshot.agents[0].agent_status;
        }
        assert_eq!(projected, AgentStatus::Idle, "{states:?}");
    }

    fn completions(boot: &str, revision: u64, seq: Option<u64>) -> EndpointAgentCompletions {
        EndpointAgentCompletions {
            unread: None,
            boot_id: boot.into(),
            revision,
            completions: seq
                .map(|seq| ("agent-pane".into(), seq))
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn completion_guard_initial_unknown_to_idle_does_not_project_done() {
        assert_idle_sequence(&[(AgentStatus::Unknown, 0), (AgentStatus::Idle, 1)]);
    }

    #[test]
    fn completion_guard_new_idle_agent_is_not_completed_work() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut baseline = snapshot(AgentStatus::Idle, 0, 1);
        baseline.agents.clear();
        presentation.project_snapshot(&mut baseline);
        let mut ready = snapshot(AgentStatus::Idle, 1, 2);

        presentation.project_snapshot(&mut ready);

        assert_eq!(ready.agents[0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn completion_guard_session_rebind_does_not_project_done() {
        assert_idle_sequence(&[
            (AgentStatus::Idle, 4),
            (AgentStatus::Unknown, 5),
            (AgentStatus::Idle, 6),
        ]);
    }

    #[test]
    fn completion_guard_startup_blocker_does_not_project_done() {
        assert_idle_sequence(&[
            (AgentStatus::Unknown, 0),
            (AgentStatus::Blocked, 1),
            (AgentStatus::Idle, 2),
        ]);
    }

    #[test]
    fn completion_guard_distinguishes_coalesced_work_from_rebind() {
        for completion in [None, Some(6)] {
            let mut presentation = EndpointAgentPresentation::default();
            let mut baseline = snapshot(AgentStatus::Idle, 4, 1);
            presentation.project_snapshot(&mut baseline);
            presentation.receive_completions(None, completions(&baseline.boot_id, 2, completion));
            let mut settled = snapshot(AgentStatus::Idle, 6, 2);

            presentation.project_snapshot(&mut settled);

            let expected = if completion.is_some() {
                AgentStatus::Done
            } else {
                AgentStatus::Idle
            };
            assert_eq!(settled.agents[0].agent_status, expected);
            presentation.project_snapshot(&mut settled);
            assert_eq!(settled.agents[0].agent_status, expected);
        }
    }

    #[test]
    fn completion_guard_server_suppression_overrides_client_observed_work() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut baseline = snapshot(AgentStatus::Working, 1, 1);
        presentation.project_snapshot(&mut baseline);
        presentation.receive_completions(None, completions(&baseline.boot_id, 2, None));
        let mut settled = snapshot(AgentStatus::Idle, 2, 2);

        presentation.project_snapshot(&mut settled);

        assert_eq!(settled.agents[0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn completion_guard_companion_is_scoped_to_boot_revision_and_connection() {
        for (boot, revision, generation, seq) in [
            ("old-boot", 2, Some(1), 6),
            ("endpoint-boot", 1, Some(1), 6),
            ("endpoint-boot", 2, Some(0), 6),
            ("endpoint-boot", 2, Some(1), 5),
        ] {
            let mut presentation = EndpointAgentPresentation::default();
            let mut baseline = snapshot(AgentStatus::Idle, 4, 1);
            presentation.project_snapshot_for_generation(&mut baseline, Some(1));
            presentation.receive_completions(generation, completions(boot, revision, Some(seq)));
            let mut settled = snapshot(AgentStatus::Idle, 6, 2);

            presentation.project_snapshot_for_generation(&mut settled, Some(1));

            assert_eq!(settled.agents[0].agent_status, AgentStatus::Idle);
        }
    }

    #[test]
    fn completion_guard_new_boot_baselines_existing_completions() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut old = snapshot(AgentStatus::Working, 1, 1);
        old.boot_id = "old-boot".into();
        presentation.project_snapshot(&mut old);
        presentation.receive_completions(None, completions("endpoint-boot", 1, Some(6)));
        let mut restored = snapshot(AgentStatus::Idle, 6, 1);

        presentation.project_snapshot(&mut restored);

        assert_eq!(restored.agents[0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn unpresented_working_completion_projects_done() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut initial = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut initial);
        let mut completed = snapshot(AgentStatus::Idle, 5, 2);

        presentation.project_snapshot(&mut completed);

        assert_eq!(completed.agents[0].agent_status, AgentStatus::Done);
    }

    #[test]
    fn pane_scope_acknowledges_only_the_focused_pane() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut initial = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut initial);
        let mut completed = snapshot(AgentStatus::Idle, 5, 2);
        presentation.project_snapshot(&mut completed);
        let mut split = surface(2);
        split.panes[0].focused = false;

        assert!(!presentation.acknowledge_surface(
            &mut completed,
            &split,
            Some(true),
            Instant::now(),
            Duration::ZERO,
            SeenScopeConfig::Pane,
        ));
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Done);
        assert!(presentation.acknowledge_surface(
            &mut completed,
            &split,
            Some(true),
            Instant::now(),
            Duration::ZERO,
            SeenScopeConfig::Tab,
        ));
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn a_held_pane_stays_done_until_it_leaves_the_screen() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut initial = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut initial);
        let mut completed = snapshot(AgentStatus::Idle, 5, 2);
        presentation.project_snapshot(&mut completed);
        let ack = |presentation: &mut EndpointAgentPresentation,
                   completed: &mut ClientShellSnapshot,
                   surface: &PaneSurfaceFrame| {
            presentation.acknowledge_surface(
                completed,
                surface,
                Some(true),
                Instant::now(),
                Duration::ZERO,
                SeenScopeConfig::Pane,
            )
        };
        assert!(!ack(&mut presentation, &mut completed, &surface(1)));
        presentation.hold_unread("agent-pane".into());

        assert!(!ack(&mut presentation, &mut completed, &surface(2)));
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Done);

        let mut elsewhere = surface(2);
        elsewhere.panes[0].pane_id = "other-pane".into();
        ack(&mut presentation, &mut completed, &elsewhere);
        assert!(ack(&mut presentation, &mut completed, &surface(2)));
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Idle);
    }

    /// The server's completion for the agent, and whether someone read it.
    fn reads(revision: u64, completion: u64, read: bool) -> EndpointAgentCompletions {
        EndpointAgentCompletions {
            unread: Some(if read {
                Default::default()
            } else {
                ["agent-pane".to_owned()].into()
            }),
            ..completions("endpoint-boot", revision, Some(completion))
        }
    }

    /// A completed agent at revision 2 with sequence 5, projected done.
    fn done_presentation() -> EndpointAgentPresentation {
        let mut presentation = EndpointAgentPresentation::default();
        let mut initial = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut initial);
        presentation.receive_completions(None, reads(2, 5, false));
        let mut completed = snapshot(AgentStatus::Idle, 5, 2);
        presentation.project_snapshot(&mut completed);
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Done);
        presentation
    }

    #[test]
    fn server_read_clears_done_without_viewing() {
        let mut presentation = done_presentation();
        presentation.receive_completions(None, reads(3, 5, true));
        let mut read = snapshot(AgentStatus::Idle, 5, 3);

        presentation.project_snapshot(&mut read);

        assert_eq!(read.agents[0].agent_status, AgentStatus::Idle);
        assert!(presentation.take_unsent_reads().is_empty());
    }

    #[test]
    fn older_server_read_leaves_a_newer_completion_done() {
        let mut presentation = done_presentation();
        presentation.receive_completions(None, reads(3, 5, true));
        let mut read = snapshot(AgentStatus::Idle, 5, 3);
        presentation.project_snapshot(&mut read);
        assert_eq!(read.agents[0].agent_status, AgentStatus::Idle);

        // The agent finished again; the read of completion 5 does not cover 6.
        presentation.receive_completions(None, reads(4, 6, false));
        let mut newer = snapshot(AgentStatus::Idle, 6, 4);
        presentation.project_snapshot(&mut newer);

        assert_eq!(newer.agents[0].agent_status, AgentStatus::Done);
    }

    #[test]
    fn server_read_never_lowers_the_local_acknowledgement() {
        let mut presentation = done_presentation();
        let mut completed = snapshot(AgentStatus::Idle, 5, 3);
        presentation.receive_completions(None, reads(3, 5, false));
        presentation.project_snapshot(&mut completed);
        assert!(presentation.acknowledge_surface(
            &mut completed,
            &surface(3),
            Some(true),
            Instant::now(),
            Duration::ZERO,
            SeenScopeConfig::Pane,
        ));
        // The server has not heard of the read yet and reports none.
        presentation.receive_completions(None, reads(4, 5, false));
        let mut next = snapshot(AgentStatus::Idle, 5, 4);

        presentation.project_snapshot(&mut next);

        assert_eq!(next.agents[0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn a_new_boot_drops_server_reads() {
        let mut presentation = done_presentation();
        presentation.receive_completions(None, reads(3, 5, true));
        let mut read = snapshot(AgentStatus::Idle, 5, 3);
        presentation.project_snapshot(&mut read);
        let mut restarted = snapshot(AgentStatus::Working, 1, 1);
        restarted.boot_id = "new-boot".into();
        presentation.project_snapshot(&mut restarted);
        assert!(presentation.server_reads.is_empty());

        // Completions after the restart are done until read on the new boot.
        let mut completed = snapshot(AgentStatus::Idle, 5, 2);
        completed.boot_id = "new-boot".into();
        presentation.project_snapshot(&mut completed);
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Done);
    }

    #[test]
    fn a_read_from_elsewhere_wins_over_a_local_hold() {
        let mut presentation = done_presentation();
        presentation.hold_unread("agent-pane".into());
        let mut held = snapshot(AgentStatus::Idle, 5, 3);
        presentation.receive_completions(None, reads(3, 5, false));
        presentation.project_snapshot(&mut held);
        assert!(!presentation.acknowledge_surface(
            &mut held,
            &surface(3),
            Some(true),
            Instant::now(),
            Duration::ZERO,
            SeenScopeConfig::Pane,
        ));
        assert_eq!(held.agents[0].agent_status, AgentStatus::Done);

        presentation.receive_completions(None, reads(4, 5, true));
        let mut read = snapshot(AgentStatus::Idle, 5, 4);
        presentation.project_snapshot(&mut read);

        assert_eq!(read.agents[0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn acknowledging_a_completion_reports_one_read() {
        let mut presentation = done_presentation();
        let mut completed = snapshot(AgentStatus::Idle, 5, 3);
        presentation.receive_completions(None, reads(3, 5, false));
        presentation.project_snapshot(&mut completed);
        assert!(presentation.take_unsent_reads().is_empty());
        let ack = |presentation: &mut EndpointAgentPresentation,
                   completed: &mut ClientShellSnapshot| {
            presentation.acknowledge_surface(
                completed,
                &surface(3),
                Some(true),
                Instant::now(),
                Duration::ZERO,
                SeenScopeConfig::Pane,
            )
        };

        assert!(ack(&mut presentation, &mut completed));
        assert_eq!(
            presentation.take_unsent_reads(),
            vec![("agent-pane".to_owned(), 5)]
        );
        assert!(!ack(&mut presentation, &mut completed));
        assert!(presentation.take_unsent_reads().is_empty());
    }

    #[test]
    fn acknowledging_a_completion_the_server_already_read_reports_nothing() {
        let mut presentation = done_presentation();
        let mut read = snapshot(AgentStatus::Idle, 5, 3);
        presentation.receive_completions(None, reads(3, 5, true));
        presentation.project_snapshot(&mut read);

        presentation.acknowledge_surface(
            &mut read,
            &surface(3),
            Some(true),
            Instant::now(),
            Duration::ZERO,
            SeenScopeConfig::Pane,
        );

        assert!(presentation.take_unsent_reads().is_empty());
    }

    #[test]
    fn coherent_presented_surface_acknowledges_completion() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut initial = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut initial);
        let mut completed = snapshot(AgentStatus::Idle, 5, 2);
        presentation.project_snapshot(&mut completed);

        assert!(presentation.acknowledge_surface(
            &mut completed,
            &surface(2),
            Some(true),
            Instant::now(),
            Duration::ZERO,
            SeenScopeConfig::Pane
        ));
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn clients_acknowledge_the_same_endpoint_completion_independently() {
        let mut viewing_client = EndpointAgentPresentation::default();
        let mut background_client = EndpointAgentPresentation::default();
        let mut initial_for_viewer = snapshot(AgentStatus::Working, 4, 1);
        let mut initial_for_background = initial_for_viewer.clone();
        viewing_client.project_snapshot(&mut initial_for_viewer);
        background_client.project_snapshot(&mut initial_for_background);
        let mut completed_for_viewer = snapshot(AgentStatus::Idle, 5, 2);
        let mut completed_for_background = completed_for_viewer.clone();
        viewing_client.project_snapshot(&mut completed_for_viewer);
        background_client.project_snapshot(&mut completed_for_background);

        assert!(viewing_client.acknowledge_surface(
            &mut completed_for_viewer,
            &surface(2),
            Some(true),
            Instant::now(),
            Duration::ZERO,
            SeenScopeConfig::Pane,
        ));

        assert_eq!(
            completed_for_viewer.agents[0].agent_status,
            AgentStatus::Idle
        );
        assert_eq!(
            completed_for_background.agents[0].agent_status,
            AgentStatus::Done
        );
    }

    #[test]
    fn stale_or_unfocused_surface_does_not_acknowledge_completion() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut initial = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut initial);
        let mut completed = snapshot(AgentStatus::Idle, 5, 2);
        presentation.project_snapshot(&mut completed);

        assert!(!presentation.acknowledge_surface(
            &mut completed,
            &surface(1),
            Some(true),
            Instant::now(),
            Duration::ZERO,
            SeenScopeConfig::Pane
        ));
        assert!(!presentation.acknowledge_surface(
            &mut completed,
            &surface(2),
            Some(false),
            Instant::now(),
            Duration::ZERO,
            SeenScopeConfig::Pane
        ));
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Done);
    }

    #[test]
    fn seen_delay_keeps_completion_done_until_it_has_been_shown_long_enough() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut initial = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut initial);
        let mut completed = snapshot(AgentStatus::Idle, 5, 2);
        presentation.project_snapshot(&mut completed);
        let delay = Duration::from_millis(500);
        let start = Instant::now();

        assert!(!presentation.acknowledge_surface(
            &mut completed,
            &surface(2),
            Some(true),
            start,
            delay,
            SeenScopeConfig::Pane
        ));
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Done);
        assert_eq!(presentation.seen_deadline(), Some(start + delay));

        let early = start + Duration::from_millis(499);
        assert!(!presentation.acknowledge_surface(
            &mut completed,
            &surface(2),
            Some(true),
            early,
            delay,
            SeenScopeConfig::Pane
        ));
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Done);

        assert!(presentation.acknowledge_surface(
            &mut completed,
            &surface(2),
            Some(true),
            start + delay,
            delay,
            SeenScopeConfig::Pane
        ));
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Idle);
        assert_eq!(presentation.seen_deadline(), None);
    }

    #[test]
    fn seen_delay_restarts_when_the_pane_leaves_the_screen() {
        let mut presentation = EndpointAgentPresentation::default();
        let mut initial = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut initial);
        let mut completed = snapshot(AgentStatus::Idle, 5, 2);
        presentation.project_snapshot(&mut completed);
        let delay = Duration::from_millis(500);
        let start = Instant::now();
        let mut elsewhere = surface(2);
        elsewhere.panes.clear();

        presentation.acknowledge_surface(
            &mut completed,
            &surface(2),
            Some(true),
            start,
            delay,
            SeenScopeConfig::Pane,
        );
        let away = start + Duration::from_millis(300);
        presentation.acknowledge_surface(
            &mut completed,
            &elsewhere,
            Some(true),
            away,
            delay,
            SeenScopeConfig::Pane,
        );
        let back = start + Duration::from_millis(600);
        assert!(!presentation.acknowledge_surface(
            &mut completed,
            &surface(2),
            Some(true),
            back,
            delay,
            SeenScopeConfig::Pane
        ));
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Done);
        assert!(presentation.acknowledge_surface(
            &mut completed,
            &surface(2),
            Some(true),
            back + delay,
            delay,
            SeenScopeConfig::Pane
        ));
        assert_eq!(completed.agents[0].agent_status, AgentStatus::Idle);
    }
}
