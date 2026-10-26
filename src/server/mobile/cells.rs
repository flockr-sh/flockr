//! Cell frames → `surface.frame` / `surface.delta` JSON (§10.3–§10.5). Pure.
//!
//! Runs off the server loop, in the gateway's per-subscription task.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::protocol::{CellData, FrameData, PaneSurfaceFrame};

/// Style table size that forces a full frame.
pub(crate) const MAX_STYLES: usize = 4096;
/// Link table size that forces a full frame.
pub(crate) const MAX_LINKS: usize = 256;
/// Default-style spaces kept inside a run before it is split into a gap.
const MAX_INLINE_GAP: usize = 3;
const TRUECOLOR_BASE: i64 = 16_777_216;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct StyleKey {
    fg: i64,
    bg: i64,
    flags: u32,
    link: Option<u32>,
}

const DEFAULT_STYLE: StyleKey = StyleKey {
    fg: -1,
    bg: -1,
    flags: 0,
    link: None,
};

/// Maps a packed wire color (`color_to_u32`) to the mobile color integer.
pub(crate) fn mobile_color(value: u32) -> i64 {
    match value >> 24 {
        0x00 => match value & 0xFF {
            1..=16 => i64::from(value & 0xFF) - 1,
            _ => -1,
        },
        0x01 => i64::from(value & 0xFF),
        0x02 => TRUECOLOR_BASE + i64::from(value & 0x00FF_FFFF),
        _ => -1,
    }
}

/// Modifier bits 0–8 plus underline style in bits 9–12.
pub(crate) fn mobile_flags(modifier: u16) -> u32 {
    u32::from(modifier & 0x1FF)
        | (u32::from(crate::protocol::underline_style_from_modifier(modifier) & 0x0F) << 9)
}

fn cell_is_default_space(cell: &CellData) -> bool {
    (cell.symbol.is_empty() || cell.symbol == " ")
        && mobile_color(cell.fg) == -1
        && mobile_color(cell.bg) == -1
        && mobile_flags(cell.modifier) == 0
        && cell.hyperlink.is_none()
}

/// A single BMP code point of display width 1.
fn simple_char(symbol: &str) -> Option<char> {
    let mut chars = symbol.chars();
    let ch = chars.next()?;
    if chars.next().is_some() || u32::from(ch) > 0xFFFF {
        return None;
    }
    (UnicodeWidthChar::width(ch) == Some(1)).then_some(ch)
}

fn push_json_str(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if u32::from(ch) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", u32::from(ch)));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

fn link_uri<'a>(frame: &'a FrameData, cell: &CellData) -> Option<&'a str> {
    cell.hyperlink
        .and_then(|index| frame.hyperlinks.get(index as usize))
        .map(String::as_str)
}

fn row_hash(frame: &FrameData, y: usize) -> u64 {
    let width = usize::from(frame.width);
    let mut hasher = DefaultHasher::new();
    for cell in &frame.cells[y * width..(y + 1) * width] {
        cell.symbol.hash(&mut hasher);
        cell.fg.hash(&mut hasher);
        cell.bg.hash(&mut hasher);
        cell.modifier.hash(&mut hasher);
        link_uri(frame, cell).hash(&mut hasher);
    }
    hasher.finish()
}

fn rows_equal(a: &FrameData, ay: usize, b: &FrameData, by: usize) -> bool {
    let width = usize::from(a.width);
    if width != usize::from(b.width) {
        return false;
    }
    let left = &a.cells[ay * width..(ay + 1) * width];
    let right = &b.cells[by * width..(by + 1) * width];
    left.iter().zip(right).all(|(l, r)| {
        l.symbol == r.symbol
            && l.fg == r.fg
            && l.bg == r.bg
            && l.modifier == r.modifier
            && link_uri(a, l) == link_uri(b, r)
    })
}

/// Picks a whole-grid upward shift `n` (§10.5), or 0.
pub(crate) fn detect_shift(old: &[u64], new: &[u64]) -> usize {
    let rows = new.len();
    if old.len() != rows || rows < 2 {
        return 0;
    }
    let same = old.iter().zip(new).filter(|(a, b)| a == b).count();
    if same * 2 >= rows {
        return 0;
    }
    for shift in 1..=rows / 2 {
        let overlap = rows - shift;
        let matches = (0..overlap).filter(|&y| old[y + shift] == new[y]).count();
        if matches * 2 >= overlap && matches > same {
            return shift;
        }
    }
    0
}

/// One encoded `surface.frame` or `surface.delta` message.
#[derive(Debug)]
pub(crate) struct Encoded {
    pub json: String,
}

#[cfg(test)]
impl Encoded {
    fn full(&self) -> bool {
        self.json.starts_with(r#"{"t":"surface.frame""#)
    }
}

/// Per-subscription encoder state: style and link tables, last frame, `seq`.
pub(crate) struct SurfaceEncoder {
    sub: u32,
    seq: u64,
    shift_enabled: bool,
    styles: Vec<StyleKey>,
    style_ids: HashMap<StyleKey, u32>,
    styles_sent: usize,
    links: Vec<String>,
    link_ids: HashMap<String, u32>,
    links_sent: usize,
    last: Option<Arc<PaneSurfaceFrame>>,
    last_hashes: Vec<u64>,
    run_text: String,
}

struct Meta {
    cursor: String,
    scroll: String,
    alt: bool,
}

fn meta_of(surface: &PaneSurfaceFrame) -> Meta {
    let cursor = match &surface.frame.cursor {
        Some(cursor) => format!(
            r#"{{"x":{},"y":{},"visible":{},"shape":{}}}"#,
            cursor.x, cursor.y, cursor.visible, cursor.shape
        ),
        None => "null".to_owned(),
    };
    let pane = surface.panes.first();
    let scroll = match pane.and_then(|pane| pane.scroll) {
        Some(scroll) => format!(
            r#"{{"offset":{},"max":{},"viewport":{}}}"#,
            scroll.offset_from_bottom, scroll.max_offset_from_bottom, scroll.viewport_rows
        ),
        None => "null".to_owned(),
    };
    Meta {
        cursor,
        scroll,
        alt: pane.is_some_and(|pane| pane.alternate_screen_active),
    }
}

impl SurfaceEncoder {
    pub(crate) fn new(sub: u32, shift_enabled: bool) -> Self {
        let mut encoder = Self {
            sub,
            seq: 0,
            shift_enabled,
            styles: Vec::new(),
            style_ids: HashMap::new(),
            styles_sent: 0,
            links: Vec::new(),
            link_ids: HashMap::new(),
            links_sent: 0,
            last: None,
            last_hashes: Vec::new(),
            run_text: String::new(),
        };
        encoder.reset_tables();
        encoder
    }

    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    /// The next encode sends a full frame.
    pub(crate) fn reset(&mut self) {
        self.last = None;
        self.last_hashes.clear();
    }

    fn reset_tables(&mut self) {
        self.styles.clear();
        self.style_ids.clear();
        self.styles.push(DEFAULT_STYLE);
        self.style_ids.insert(DEFAULT_STYLE, 0);
        self.styles_sent = 0;
        self.links.clear();
        self.link_ids.clear();
        self.links_sent = 0;
    }

    fn link_id(&mut self, uri: &str) -> u32 {
        if let Some(id) = self.link_ids.get(uri) {
            return *id;
        }
        let id = self.links.len() as u32;
        self.links.push(uri.to_owned());
        self.link_ids.insert(uri.to_owned(), id);
        id
    }

    fn style_id(&mut self, frame: &FrameData, cell: &CellData) -> u32 {
        let link = link_uri(frame, cell).map(|uri| self.link_id(uri));
        let key = StyleKey {
            fg: mobile_color(cell.fg),
            bg: mobile_color(cell.bg),
            flags: mobile_flags(cell.modifier),
            link,
        };
        if let Some(id) = self.style_ids.get(&key) {
            return *id;
        }
        let id = self.styles.len() as u32;
        self.styles.push(key);
        self.style_ids.insert(key, id);
        id
    }

    fn flush_run(&mut self, out: &mut String, first: &mut bool, run: &mut Option<(usize, u32)>) {
        let Some((x, style)) = run.take() else {
            return;
        };
        if !*first {
            out.push(',');
        }
        *first = false;
        out.push_str(&format!("[{x},{style},"));
        if style == 0 {
            // Default-style spaces only join style-0 runs; never end on them.
            let trimmed = self.run_text.trim_end_matches(' ').len();
            self.run_text.truncate(trimmed);
        }
        let text = std::mem::take(&mut self.run_text);
        push_json_str(out, &text);
        self.run_text = text;
        self.run_text.clear();
        out.push(']');
    }

    fn write_row(&mut self, out: &mut String, frame: &FrameData, y: usize) {
        let width = usize::from(frame.width);
        let row_start = y * width;
        out.push('[');
        let mut first = true;
        let mut run: Option<(usize, u32)> = None;
        self.run_text.clear();
        let mut x = 0;
        while x < width {
            let cell = &frame.cells[row_start + x];
            if cell_is_default_space(cell) {
                let mut gap = 1;
                while x + gap < width && cell_is_default_space(&frame.cells[row_start + x + gap]) {
                    gap += 1;
                }
                if x + gap == width {
                    break;
                }
                let joins = matches!(run, Some((start, 0)) if start + self.run_text.encode_utf16().count() == x);
                if joins && gap <= MAX_INLINE_GAP {
                    for _ in 0..gap {
                        self.run_text.push(' ');
                    }
                } else {
                    self.flush_run(out, &mut first, &mut run);
                }
                x += gap;
                continue;
            }
            let symbol = if cell.symbol.is_empty() {
                " "
            } else {
                cell.symbol.as_str()
            };
            let style = self.style_id(frame, cell);
            if let Some(ch) = simple_char(symbol) {
                let joins = matches!(run, Some((start, run_style)) if run_style == style
                    && start + self.run_text.encode_utf16().count() == x);
                if !joins {
                    self.flush_run(out, &mut first, &mut run);
                    run = Some((x, style));
                }
                self.run_text.push(ch);
                x += 1;
            } else {
                self.flush_run(out, &mut first, &mut run);
                let cols = UnicodeWidthStr::width(symbol).clamp(1, 2).min(width - x);
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&format!("[{x},{style},"));
                push_json_str(out, symbol);
                out.push_str(&format!(",{cols}]"));
                x += cols;
            }
        }
        self.flush_run(out, &mut first, &mut run);
        out.push(']');
    }

    fn write_styles(&self, out: &mut String, from: usize) {
        out.push('[');
        for (index, style) in self.styles[from..].iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            match style.link {
                Some(link) => out.push_str(&format!(
                    "[{},{},{},{}]",
                    style.fg, style.bg, style.flags, link
                )),
                None => out.push_str(&format!("[{},{},{}]", style.fg, style.bg, style.flags)),
            }
        }
        out.push(']');
    }

    fn write_links(&self, out: &mut String, from: usize) {
        out.push('[');
        for (index, link) in self.links[from..].iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            push_json_str(out, link);
        }
        out.push(']');
    }

    fn encode_full(&mut self, surface: &Arc<PaneSurfaceFrame>, hashes: Vec<u64>) -> Encoded {
        let frame = &surface.frame;
        self.reset_tables();
        let mut lines = String::with_capacity(usize::from(frame.height) * 32);
        lines.push('[');
        for y in 0..usize::from(frame.height) {
            if y > 0 {
                lines.push(',');
            }
            self.write_row(&mut lines, frame, y);
        }
        lines.push(']');
        let meta = meta_of(surface);
        self.seq += 1;
        let mut out = String::with_capacity(lines.len() + 256);
        out.push_str(&format!(
            r#"{{"t":"surface.frame","sub":{},"seq":{},"cols":{},"rows":{},"styles":"#,
            self.sub, self.seq, frame.width, frame.height
        ));
        self.write_styles(&mut out, 0);
        if !self.links.is_empty() {
            out.push_str(r#","links":"#);
            self.write_links(&mut out, 0);
        }
        out.push_str(r#","lines":"#);
        out.push_str(&lines);
        out.push_str(&format!(
            r#","cursor":{},"scroll":{},"alt":{}}}"#,
            meta.cursor, meta.scroll, meta.alt
        ));
        self.styles_sent = self.styles.len();
        self.links_sent = self.links.len();
        self.last = Some(surface.clone());
        self.last_hashes = hashes;
        Encoded { json: out }
    }

    /// Encodes `surface` against the last sent one. `None` when nothing changed.
    pub(crate) fn encode(&mut self, surface: &Arc<PaneSurfaceFrame>) -> Option<Encoded> {
        let frame = &surface.frame;
        let rows = usize::from(frame.height);
        if frame.cells.len() != usize::from(frame.width) * rows {
            return None;
        }
        let hashes = (0..rows).map(|y| row_hash(frame, y)).collect::<Vec<_>>();
        let Some(last) = self.last.clone() else {
            return Some(self.encode_full(surface, hashes));
        };
        if last.frame.width != frame.width || last.frame.height != frame.height {
            return Some(self.encode_full(surface, hashes));
        }
        let shift = if self.shift_enabled {
            detect_shift(&self.last_hashes, &hashes)
        } else {
            0
        };
        let changed = (0..rows)
            .filter(|&y| {
                let Some(old_y) = y.checked_add(shift).filter(|old_y| *old_y < rows) else {
                    return true;
                };
                self.last_hashes[old_y] != hashes[y] || !rows_equal(&last.frame, old_y, frame, y)
            })
            .collect::<Vec<_>>();
        let meta = meta_of(surface);
        let last_meta = meta_of(&last);
        if changed.is_empty()
            && shift == 0
            && meta.cursor == last_meta.cursor
            && meta.scroll == last_meta.scroll
            && meta.alt == last_meta.alt
        {
            return None;
        }
        if shift == 0 && changed.len() == rows {
            return Some(self.encode_full(surface, hashes));
        }
        let mut lines = String::new();
        lines.push('[');
        for (index, y) in changed.iter().enumerate() {
            if index > 0 {
                lines.push(',');
            }
            lines.push_str(&format!("[{y},"));
            self.write_row(&mut lines, frame, *y);
            lines.push(']');
        }
        lines.push(']');
        if self.styles.len() > MAX_STYLES || self.links.len() > MAX_LINKS {
            return Some(self.encode_full(surface, hashes));
        }
        let base = self.seq;
        self.seq += 1;
        let mut out = String::with_capacity(lines.len() + 256);
        out.push_str(&format!(
            r#"{{"t":"surface.delta","sub":{},"seq":{},"base":{}"#,
            self.sub, self.seq, base
        ));
        if shift > 0 {
            out.push_str(&format!(r#","shift":{shift}"#));
        }
        out.push_str(r#","styles_add":"#);
        self.write_styles(&mut out, self.styles_sent);
        out.push_str(r#","links_add":"#);
        self.write_links(&mut out, self.links_sent);
        out.push_str(r#","lines":"#);
        out.push_str(&lines);
        out.push_str(&format!(
            r#","cursor":{},"scroll":{},"alt":{}}}"#,
            meta.cursor, meta.scroll, meta.alt
        ));
        self.styles_sent = self.styles.len();
        self.links_sent = self.links.len();
        self.last = Some(surface.clone());
        self.last_hashes = hashes;
        Some(Encoded { json: out })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{CursorState, PaneSurfacePane, PaneSurfaceScrollMetrics, SurfaceRect};
    use serde_json::Value;

    fn cell(symbol: &str) -> CellData {
        CellData {
            symbol: symbol.to_owned(),
            fg: 0,
            bg: 0,
            modifier: 0,
            skip: false,
            hyperlink: None,
        }
    }

    fn styled(symbol: &str, fg: u32, bg: u32, modifier: u16) -> CellData {
        CellData {
            fg,
            bg,
            modifier,
            ..cell(symbol)
        }
    }

    fn frame_from_rows(width: u16, rows: &[Vec<CellData>]) -> FrameData {
        let mut cells = Vec::new();
        for row in rows {
            let mut row = row.clone();
            row.resize(usize::from(width), cell(" "));
            cells.extend(row);
        }
        FrameData {
            cells,
            width,
            height: rows.len() as u16,
            cursor: Some(CursorState {
                x: 2,
                y: rows.len() as u16 - 1,
                visible: true,
                shape: 0,
            }),
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
        }
    }

    fn text(content: &str) -> Vec<CellData> {
        content.chars().map(|ch| cell(&ch.to_string())).collect()
    }

    fn surface(frame: FrameData, max: u64) -> Arc<PaneSurfaceFrame> {
        let rect = SurfaceRect {
            x: 0,
            y: 0,
            width: frame.width,
            height: frame.height,
        };
        Arc::new(PaneSurfaceFrame {
            boot_id: String::new(),
            projection_revision: 0,
            surface_revision: 0,
            panes: vec![PaneSurfacePane {
                pane_id: String::new(),
                content_revision: 0,
                rect,
                inner_rect: rect,
                scrollbar_rect: None,
                scroll: Some(PaneSurfaceScrollMetrics {
                    offset_from_bottom: 0,
                    max_offset_from_bottom: max,
                    viewport_rows: u64::from(frame.height),
                }),
                focused: false,
                mouse_reporting: false,
                sgr_pixel_mouse: false,
                alternate_screen_active: false,
                pixel_width: 0,
                pixel_height: 0,
            }],
            splits: Vec::new(),
            popup: None,
            graphics: Default::default(),
            frame,
        })
    }

    fn fixture(name: &str) -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/mobile-v1")
                    .join(name),
            )
            .unwrap(),
        )
        .unwrap()
    }

    /// Row 0 of the spec example: "$ ls", bold green "main.rs", wide 漢, red bg spaces.
    fn example_row() -> Vec<CellData> {
        let mut row = text("$ ls ");
        row.extend(
            "main.rs"
                .chars()
                .map(|ch| styled(&ch.to_string(), 0x03, 0, 1)),
        );
        row.push(cell(" "));
        row.push(cell("漢"));
        row.push(cell(" "));
        row.extend((0..5).map(|_| styled(" ", 0, 0x02, 0)));
        row
    }

    fn example_frame() -> FrameData {
        let mut frame = frame_from_rows(20, &[example_row(), vec![], text("ok")]);
        frame.hyperlinks = vec!["https://github.com/flockr-sh/flockr".to_owned()];
        frame
    }

    #[test]
    fn colors_and_flags_map_to_the_mobile_integers() {
        assert_eq!(mobile_color(0), -1);
        assert_eq!(mobile_color(0x01), 0);
        assert_eq!(mobile_color(0x03), 2);
        assert_eq!(mobile_color(0x10), 15);
        assert_eq!(mobile_color(0x01_00_00_c4), 196);
        assert_eq!(mobile_color(0x02_12_34_56), 16_777_216 + 0x12_34_56);
        assert_eq!(mobile_flags(1 | 64), 65);
        assert_eq!(mobile_flags(8 | (3 << 12)), 8 | (3 << 9));
    }

    #[test]
    fn full_frame_matches_the_fixture() {
        let mut encoder = SurfaceEncoder::new(1, true);
        let encoded = encoder.encode(&surface(example_frame(), 1834)).unwrap();
        assert!(encoded.full());
        assert_eq!(encoder.seq(), 1);
        let mut value: Value = serde_json::from_str(&encoded.json).unwrap();
        let mut expected = fixture("server.surface.frame.json");
        // The example frame has a link table entry but no linked cells.
        expected.as_object_mut().unwrap().remove("links");
        value.as_object_mut().unwrap().remove("links");
        assert_eq!(value, expected);
    }

    #[test]
    fn delta_with_shift_matches_the_fixture() {
        let mut encoder = SurfaceEncoder::new(1, true);
        encoder.encode(&surface(example_frame(), 1834)).unwrap();
        // Scroll up one row: old rows 1..3 move to 0..2, new content below.
        let mut next = frame_from_rows(
            20,
            &[
                vec![],
                "test result: ok"
                    .chars()
                    .map(|ch| styled(&ch.to_string(), 0x02, 0, 0))
                    .collect(),
                text("$ "),
            ],
        );
        next.cursor = Some(CursorState {
            x: 2,
            y: 2,
            visible: true,
            shape: 0,
        });
        let encoded = encoder.encode(&surface(next, 1835)).unwrap();
        assert!(!encoded.full());
        let value: Value = serde_json::from_str(&encoded.json).unwrap();
        assert_eq!(value, fixture("server.surface.delta.json"));
    }

    #[test]
    fn unchanged_frames_send_nothing_and_resync_sends_full() {
        let mut encoder = SurfaceEncoder::new(4, false);
        let first = surface(example_frame(), 10);
        assert!(encoder.encode(&first).unwrap().full());
        assert!(encoder.encode(&surface(example_frame(), 10)).is_none());
        let moved_cursor = {
            let mut frame = example_frame();
            frame.cursor = None;
            surface(frame, 10)
        };
        let delta = encoder.encode(&moved_cursor).unwrap();
        assert!(!delta.full());
        let value: Value = serde_json::from_str(&delta.json).unwrap();
        assert_eq!(value["lines"], Value::Array(vec![]));
        assert_eq!(value["cursor"], Value::Null);
        assert_eq!(value["base"], 1);
        encoder.reset();
        let full = encoder.encode(&moved_cursor).unwrap();
        assert!(full.full());
        assert_eq!(encoder.seq(), 3);
    }

    #[test]
    fn rows_drop_trailing_blanks_and_split_long_gaps() {
        let mut encoder = SurfaceEncoder::new(1, false);
        let mut row = text("a");
        row.extend(text("     "));
        row.extend(text("b  c"));
        let frame = frame_from_rows(30, &[row]);
        let encoded = encoder.encode(&surface(frame, 0)).unwrap();
        let value: Value = serde_json::from_str(&encoded.json).unwrap();
        assert_eq!(
            value["lines"],
            serde_json::json!([[[0, 0, "a"], [6, 0, "b  c"]]])
        );
    }

    #[test]
    fn complex_graphemes_use_the_four_element_form() {
        let mut encoder = SurfaceEncoder::new(1, false);
        let row = vec![
            cell("e\u{301}"),
            cell("👍"),
            cell(" "),
            cell("x"),
            cell("\""),
        ];
        let frame = frame_from_rows(10, &[row]);
        let encoded = encoder.encode(&surface(frame, 0)).unwrap();
        let value: Value = serde_json::from_str(&encoded.json).unwrap();
        assert_eq!(
            value["lines"],
            serde_json::json!([[[0, 0, "e\u{301}", 1], [1, 0, "👍", 2], [3, 0, "x\""]]])
        );
    }

    #[test]
    fn hyperlinks_get_a_style_with_a_link_index() {
        let mut encoder = SurfaceEncoder::new(1, false);
        let mut linked = cell("L");
        linked.hyperlink = Some(0);
        let mut frame = frame_from_rows(4, &[vec![linked]]);
        frame.hyperlinks = vec!["https://flockr.sh".to_owned()];
        let encoded = encoder.encode(&surface(frame, 0)).unwrap();
        let value: Value = serde_json::from_str(&encoded.json).unwrap();
        assert_eq!(value["links"], serde_json::json!(["https://flockr.sh"]));
        assert_eq!(value["styles"][1], serde_json::json!([-1, -1, 0, 0]));
        assert_eq!(value["lines"][0], serde_json::json!([[0, 1, "L"]]));
    }

    #[test]
    fn shift_detection_prefers_no_shift_for_mostly_static_screens() {
        assert_eq!(detect_shift(&[1, 2, 3, 4], &[1, 2, 3, 9]), 0);
        assert_eq!(detect_shift(&[1, 2, 3, 4], &[2, 3, 4, 9]), 1);
        assert_eq!(detect_shift(&[1, 2, 3, 4, 5, 6], &[3, 4, 5, 6, 7, 8]), 2);
        assert_eq!(detect_shift(&[1, 2, 3, 4], &[7, 8, 9, 10]), 0);
    }

    #[test]
    fn size_change_sends_a_full_frame() {
        let mut encoder = SurfaceEncoder::new(1, true);
        encoder
            .encode(&surface(frame_from_rows(10, &[text("a")]), 0))
            .unwrap();
        let encoded = encoder
            .encode(&surface(frame_from_rows(12, &[text("a")]), 0))
            .unwrap();
        assert!(encoded.full());
    }

    #[test]
    fn large_frames_encode_with_bounded_output() {
        for (cols, rows) in [(120_u16, 40_usize), (300, 80)] {
            let make = |offset: usize| {
                let lines = (0..rows)
                    .map(|y| {
                        format!("line {} {}", y + offset, "x".repeat(usize::from(cols) / 2))
                            .chars()
                            .take(usize::from(cols))
                            .map(|ch| {
                                styled(&ch.to_string(), 0x02_00_80_ff, 0, ((y + offset) % 2) as u16)
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                frame_from_rows(cols, &lines)
            };
            let mut encoder = SurfaceEncoder::new(1, true);
            let full = encoder.encode(&surface(make(0), 0)).unwrap();
            let scrolled = encoder.encode(&surface(make(1), 0)).unwrap();
            assert!(!scrolled.full());
            let value: Value = serde_json::from_str(&scrolled.json).unwrap();
            assert_eq!(value["shift"], 1);
            assert_eq!(value["lines"].as_array().unwrap().len(), 1);
            assert!(scrolled.json.len() * 10 < full.json.len());
        }
    }
}
