use super::*;
use crate::api::schema::{PaneLink, PaneLinkActivateParams, PaneLinkListParams};
use crate::protocol::{color_to_u32, CellData};
use crossterm::event::{KeyCode, KeyModifiers};

/// Home-row label keys. Lowercase only: shift+label copies instead of opening.
const HINT_KEYS: [char; 9] = ['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l'];

/// Labels for the focused pane's visible links, shown while in hint mode.
pub(super) struct LinkHints {
    pane_id: String,
    content_revision: u64,
    offset_from_bottom: Option<u64>,
    links: Vec<(String, PaneLink)>,
    typed: String,
}

/// `count` labels of equal length, so no label is a prefix of another.
fn hint_labels(count: usize) -> Vec<String> {
    if count <= HINT_KEYS.len() {
        return HINT_KEYS.iter().take(count).map(char::to_string).collect();
    }
    HINT_KEYS
        .iter()
        .flat_map(|first| {
            HINT_KEYS
                .iter()
                .map(move |second| format!("{first}{second}"))
        })
        .take(count)
        .collect()
}

impl ClientShellState {
    fn focused_pane_offset(&self, pane_id: &str) -> Option<Option<u64>> {
        self.hits
            .panes
            .iter()
            .find(|hit| hit.pane_id == pane_id)
            .map(|hit| hit.scroll.map(|metrics| metrics.offset_from_bottom as u64))
    }

    /// Asks the server for the focused pane's visible links.
    pub(super) fn request_link_hints(&mut self, outcome: &mut ClientShellInput) {
        let Some(pane_id) = self.focused_pane_id() else {
            return;
        };
        let Some(offset_from_bottom) = self.focused_pane_offset(&pane_id) else {
            return;
        };
        self.push_endpoint_method_with_kind(
            crate::api::schema::Method::PaneLinkList(PaneLinkListParams {
                pane_id: pane_id.clone(),
                offset_from_bottom,
            }),
            PendingEndpointKind::PaneLinkList {
                pane_id,
                offset_from_bottom,
            },
            outcome,
        );
    }

    pub(super) fn complete_link_hints(
        &mut self,
        pane_id: String,
        offset_from_bottom: Option<u64>,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> bool {
        let Ok(crate::api::schema::ResponseResult::PaneLinkList {
            content_revision,
            links,
        }) = result
        else {
            return false;
        };
        if self.mode != ClientShellMode::Terminal
            || self.overlay.is_some()
            || self.focused_pane_id().as_deref() != Some(pane_id.as_str())
            || self.focused_pane_offset(&pane_id) != Some(offset_from_bottom)
        {
            return false;
        }
        if links.is_empty() {
            return self.push_endpoint_notice(
                ClientEndpointNoticeKind::Info,
                "link_hints_empty",
                "No links",
                "No links are visible in this pane.",
            );
        }
        // The newest output is at the bottom, so the bottom links get the first labels.
        let labels = hint_labels(links.len());
        let links = links
            .into_iter()
            .rev()
            .zip(labels)
            .map(|(link, label)| (label, link));
        self.link_hints = Some(LinkHints {
            pane_id,
            content_revision,
            offset_from_bottom,
            links: links.collect(),
            typed: String::new(),
        });
        self.mode = ClientShellMode::LinkHints;
        true
    }

    pub(super) fn exit_link_hints(&mut self) {
        self.link_hints = None;
        if self.mode == ClientShellMode::LinkHints {
            self.mode = ClientShellMode::Terminal;
        }
    }

    /// Drops the labels once they no longer sit over the links they name.
    pub(super) fn invalidate_link_hints(&mut self) {
        let Some(hints) = self.link_hints.as_ref() else {
            return;
        };
        if self.focused_pane_id().as_deref() != Some(hints.pane_id.as_str())
            || self.focused_pane_offset(&hints.pane_id) != Some(hints.offset_from_bottom)
        {
            self.exit_link_hints();
        }
    }

    pub(super) fn route_link_hints_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        outcome.repaint = true;
        let Some(hints) = self.link_hints.as_mut() else {
            self.exit_link_hints();
            return;
        };
        let (ch, copy) = match key.code {
            KeyCode::Backspace => {
                hints.typed.pop();
                return;
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let copy = key.modifiers.contains(KeyModifiers::SHIFT) || ch.is_uppercase();
                (ch.to_ascii_lowercase(), copy)
            }
            _ => {
                self.exit_link_hints();
                return;
            }
        };
        let typed = format!("{}{ch}", hints.typed);
        if !hints
            .links
            .iter()
            .any(|(label, _)| label.starts_with(&typed))
        {
            return;
        }
        let Some(link) = hints
            .links
            .iter()
            .find(|(label, _)| *label == typed)
            .map(|(_, link)| link.clone())
        else {
            hints.typed = typed;
            return;
        };
        let pane_id = hints.pane_id.clone();
        let content_revision = hints.content_revision;
        let offset_from_bottom = hints.offset_from_bottom;
        self.exit_link_hints();
        if copy {
            outcome.repaint |= self.show_copy_feedback(std::time::Instant::now());
            outcome
                .actions
                .push(ClientShellAction::ClipboardWrite(link.url.into_bytes()));
            return;
        }
        // Activate through the server so plugin link handlers still apply.
        self.push_endpoint_method_with_kind(
            crate::api::schema::Method::PaneLinkActivate(PaneLinkActivateParams {
                pane_id,
                viewport_row: link.viewport_row,
                col: link.col,
                content_revision: Some(content_revision),
                offset_from_bottom,
            }),
            PendingEndpointKind::LinkHintActivate { url: link.url },
            outcome,
        );
    }

    pub(super) fn complete_link_hint_activate(
        &mut self,
        url: String,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> Vec<ClientShellAction> {
        match result {
            Ok(crate::api::schema::ResponseResult::PaneLinkActivated { handled: true, .. }) => {
                Vec::new()
            }
            // A plugin claimed the link and failed; the error notice covers it.
            Err(error) if error.code.as_deref() == Some("plugin_link_failed") => Vec::new(),
            // Otherwise open the link that was labeled, even if the pane has
            // changed since: the user saw and picked that URL.
            _ if crate::app::actions::safe_web_url(&url).is_some() => {
                vec![ClientShellAction::OpenSafeWebUrl(url)]
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn render_link_hints(
        &self,
        frame: &mut FrameData,
        occlusion: &mut crate::kitty_graphics::surface::Occlusion,
    ) {
        let Some(hints) = self.link_hints.as_ref() else {
            return;
        };
        let Some(hit) = self
            .hits
            .panes
            .iter()
            .find(|hit| hit.pane_id == hints.pane_id)
        else {
            return;
        };
        let rect = hit.inner_rect;
        let palette = &self.config.palette;
        let label_fg = match palette.panel_bg {
            ratatui::style::Color::Reset => palette.surface_dim,
            color => color,
        };
        for (label, link) in &hints.links {
            let matched = label.starts_with(&hints.typed);
            for region in &link.regions {
                if region.row >= rect.height {
                    continue;
                }
                let row = rect.y + region.row;
                let start = rect.x.saturating_add(region.start_col);
                let end = rect
                    .x
                    .saturating_add(region.end_col)
                    .min(rect.right().saturating_sub(1));
                if start > end {
                    continue;
                }
                occlusion.cover(
                    Rect::new(start, row, end - start + 1, 1).intersection(Rect::new(
                        0,
                        0,
                        frame.width,
                        frame.height,
                    )),
                );
                for col in start..=end {
                    if let Some(cell) = frame_cell(frame, col, row) {
                        if matched {
                            cell.modifier |= Modifier::UNDERLINED.bits();
                        } else {
                            cell.modifier |= Modifier::DIM.bits();
                        }
                    }
                }
            }
            if !matched {
                continue;
            }
            // Draw the label over the link's first cells, typed part dimmed.
            let row = rect.y + link.viewport_row;
            for (index, ch) in label.chars().enumerate() {
                let col = rect.x.saturating_add(link.col).saturating_add(index as u16);
                if col >= rect.right() {
                    break;
                }
                if let Some(cell) = frame_cell(frame, col, row) {
                    cell.symbol = ch.to_string();
                    cell.fg = color_to_u32(label_fg);
                    cell.bg = color_to_u32(if index < hints.typed.len() {
                        palette.overlay0
                    } else {
                        palette.accent
                    });
                    cell.modifier = Modifier::BOLD.bits();
                    cell.skip = false;
                }
            }
        }
    }
}

fn frame_cell(frame: &mut FrameData, col: u16, row: u16) -> Option<&mut CellData> {
    if col >= frame.width || row >= frame.height {
        return None;
    }
    frame
        .cells
        .get_mut(usize::from(row) * usize::from(frame.width) + usize::from(col))
}

#[cfg(test)]
mod tests {
    use super::hint_labels;

    #[test]
    fn hint_labels_are_single_keys_up_to_nine_links() {
        assert_eq!(hint_labels(3), ["a", "s", "d"]);
        assert_eq!(hint_labels(9).len(), 9);
    }

    #[test]
    fn hint_labels_switch_to_pairs_past_nine_links() {
        let labels = hint_labels(12);
        assert_eq!(labels.len(), 12);
        assert!(labels.iter().all(|label| label.len() == 2));
        assert_eq!(&labels[..2], ["aa", "as"]);
    }
}
