//! Reads a blocked agent's permission prompt from the bottom of its screen
//! (the question, the command or file it is about, and the numbered options)
//! and maps its options to the answer buttons on a push alert.
//!
//! A port of the phone app's `src/protocol/prompt.ts`; both read the same
//! fixtures (`tests/fixtures/prompts/`). Keep the two in step: the app
//! re-parses the screen before it answers and only sends a digit when its
//! parse matches what the alert carried.

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptOption {
    /// The digit that picks this option.
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prompt {
    pub question: String,
    /// The command or file the prompt is about, when the screen makes it obvious.
    pub detail: Option<String>,
    pub options: Vec<PromptOption>,
    /// Key of the option the cursor is on.
    pub selected: String,
}

const MARKERS: [char; 5] = ['❯', '›', '>', '▶', '●'];
const QUESTION_MARKERS: [char; 8] = ['❯', '›', '>', '▶', '●', '✗', '✓', '⚠'];
// Box sides and corners agents draw around a prompt.
const BOX_EDGES: [char; 11] = ['│', '┃', '║', '╭', '╮', '╰', '╯', '┌', '┐', '└', '┘'];
const SOLID_RULE: [char; 3] = ['─', '━', '═'];
const DASHED_RULE: [char; 7] = ['╌', '┄', '┈', '╍', '┅', '┉', '-'];

// Lines allowed below the options (key hints such as "Esc to cancel").
const MAX_LINES_BELOW: usize = 3;
// How far above the options the question may sit.
const MAX_QUESTION_GAP: usize = 8;
const MAX_DETAIL: usize = 300;

struct Line {
    text: String,
    indent: usize,
}

fn clean(raw: &str) -> Line {
    let replaced: String = raw
        .chars()
        .map(|c| if BOX_EDGES.contains(&c) { ' ' } else { c })
        .collect();
    let trimmed = replaced.trim_end();
    let text = trimmed.trim_start();
    let indent = trimmed.chars().count() - text.chars().count();
    Line {
        text: text.to_owned(),
        indent,
    }
}

fn is_rule(text: &str, set: &[char]) -> bool {
    let mut count = 0;
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        if !set.contains(&c) {
            return false;
        }
        count += 1;
    }
    count >= 8
}

struct OptionLine<'a> {
    /// Column of the option (marker or digit).
    col: usize,
    marker: bool,
    number: u32,
    label: &'a str,
}

/// `^(\s*)(?:([❯›>▶●])\s*)?(\d)\.\s+(.+)$` on a cleaned line at its indent.
fn option_line(line: &Line) -> Option<OptionLine<'_>> {
    let mut rest = line.text.as_str();
    let mut marker = false;
    if let Some(c) = rest.chars().next().filter(|c| MARKERS.contains(c)) {
        marker = true;
        rest = rest[c.len_utf8()..].trim_start();
    }
    let digit = rest.chars().next().filter(char::is_ascii_digit)?;
    let after = rest[1..].strip_prefix('.')?;
    if !after.starts_with(char::is_whitespace) {
        return None;
    }
    let label = after.trim_start();
    if label.is_empty() {
        return None;
    }
    Some(OptionLine {
        col: line.indent,
        marker,
        number: digit.to_digit(10)?,
        label,
    })
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Drops a key hint at the end of an option: "(esc)", "(y)", "(shift+tab)".
fn strip_key_hint(label: &str) -> &str {
    let Some(inner) = label.strip_suffix(')') else {
        return label;
    };
    let Some(open) = inner.rfind('(') else {
        return label;
    };
    let before = &inner[..open];
    let hint = inner[open + 1..].to_ascii_lowercase();
    let known = matches!(hint.as_str(), "esc" | "enter" | "tab" | "shift+tab")
        || hint.strip_prefix("ctrl+").is_some_and(|key| {
            key.len() == 1 && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        || (hint.len() == 1 && hint.chars().all(|c| c.is_ascii_alphanumeric()));
    if known && before.ends_with(char::is_whitespace) {
        before.trim_end()
    } else {
        label
    }
}

fn option_label(text: &str) -> String {
    strip_key_hint(&collapse_whitespace(text)).to_owned()
}

fn short(text: &str) -> String {
    if text.chars().count() > MAX_DETAIL {
        let mut cut: String = text.chars().take(MAX_DETAIL - 1).collect();
        cut.push('…');
        cut
    } else {
        text.to_owned()
    }
}

struct Found {
    number: u32,
    marker: bool,
    label: String,
    col: usize,
}

/// The prompt at the bottom of `screen`, or `None` when the screen does not
/// clearly show one.
pub(crate) fn parse_prompt(screen: &str) -> Option<Prompt> {
    let lines: Vec<Line> = screen.split('\n').map(clean).collect();
    let end = lines.iter().rposition(|line| !line.text.is_empty())?;

    // The last option line.
    let last = (0..=end)
        .rev()
        .find(|&i| option_line(&lines[i]).is_some())?;

    // Walk up from there: option lines, and wrapped lines indented past the
    // option number, until anything else.
    let mut found: Vec<Found> = Vec::new();
    let mut pending: Vec<&str> = Vec::new();
    let mut first = last;
    for i in (0..=last).rev() {
        let line = &lines[i];
        if line.text.is_empty() {
            break;
        }
        if let Some(option) = option_line(line) {
            let mut parts = vec![option.label];
            parts.extend(pending.drain(..).rev());
            found.insert(
                0,
                Found {
                    number: option.number,
                    marker: option.marker,
                    label: parts.join(" "),
                    col: option.col,
                },
            );
            first = i;
            continue;
        }
        if found.first().is_some_and(|next| line.indent > next.col + 1) {
            pending.push(&line.text);
            continue;
        }
        break;
    }
    // Lines after the last option that are more indented belong to it.
    let last_col = found.last()?.col;
    let mut after = last + 1;
    let mut tail = Vec::new();
    while after <= end && !lines[after].text.is_empty() && lines[after].indent > last_col + 1 {
        tail.push(lines[after].text.as_str());
        after += 1;
    }
    if let Some(last_option) = found.last_mut() {
        if !tail.is_empty() {
            last_option.label = format!("{} {}", last_option.label, tail.join(" "));
        }
    }
    let below = lines[after..=end]
        .iter()
        .filter(|line| !line.text.is_empty())
        .count();
    if below > MAX_LINES_BELOW {
        return None;
    }

    if !(2..=9).contains(&found.len()) {
        return None;
    }
    if found
        .iter()
        .enumerate()
        .any(|(i, option)| option.number as usize != i + 1)
    {
        return None;
    }
    // The cursor marks exactly one option; a plain numbered list has none.
    let mut marked = found.iter().filter(|option| option.marker);
    let selected = marked.next()?.number;
    if marked.next().is_some() {
        return None;
    }

    // The question: the nearest line above that ends with "?".
    let mut q = None;
    for i in (first.saturating_sub(MAX_QUESTION_GAP)..first).rev() {
        let text = &lines[i].text;
        if is_rule(text, &SOLID_RULE) {
            break;
        }
        if text.ends_with('?') {
            q = Some(i);
            break;
        }
    }
    let q = q?;
    let raw_question = lines[q].text.as_str();
    let question = match raw_question.chars().next() {
        Some(c) if QUESTION_MARKERS.contains(&c) => raw_question[c.len_utf8()..].trim_start(),
        _ => raw_question,
    }
    .to_owned();

    let options: Vec<PromptOption> = found
        .iter()
        .map(|option| PromptOption {
            key: option.number.to_string(),
            label: option_label(&option.label),
        })
        .collect();
    if options.iter().any(|option| option.label.is_empty()) {
        return None;
    }
    Some(Prompt {
        question,
        detail: find_detail(&lines, q, first),
        options,
        selected: selected.to_string(),
    })
}

/// The command or file the prompt is about, or `None` when unclear.
fn find_detail(lines: &[Line], q: usize, first: usize) -> Option<String> {
    // The prompt starts below the last solid rule (or box top) above the question.
    let mut top = q as isize - 1;
    while top >= 0 && q as isize - top <= 20 && !is_rule(&lines[top as usize].text, &SOLID_RULE) {
        top -= 1;
    }
    let start = if top >= 0 && q as isize - top <= 20 {
        top as usize + 1
    } else {
        q.saturating_sub(6)
    };

    // A shell command shown as "$ cmd" (Codex).
    for line in &lines[start..first] {
        if let Some(command) = line.text.strip_prefix('$') {
            if command.starts_with(char::is_whitespace) && !command.trim().is_empty() {
                return Some(short(command.trim()));
            }
        }
    }
    let above: Vec<&str> = lines[start..q]
        .iter()
        .map(|line| line.text.as_str())
        .collect();
    let filled: Vec<&str> = above.iter().copied().filter(|t| !t.is_empty()).collect();
    let title = filled.first().copied().unwrap_or("");
    // A command between two dashed rules (Claude Code's Bash prompt).
    let dashes: Vec<usize> = above
        .iter()
        .enumerate()
        .filter(|(_, text)| is_rule(text, &DASHED_RULE))
        .map(|(i, _)| i)
        .collect();
    if title.to_lowercase().contains("command") && dashes.len() >= 2 {
        let inner: Vec<&str> = above[dashes[0] + 1..dashes[1]]
            .iter()
            .copied()
            .filter(|t| !t.is_empty())
            .collect();
        if (1..=2).contains(&inner.len()) {
            return Some(short(&inner.join(" ")));
        }
    }
    // Otherwise the line under the prompt's title ("Edit file" / path).
    let second = filled.get(1)?;
    let tip = second
        .get(..4)
        .is_some_and(|start| start.eq_ignore_ascii_case("tip:"));
    if !is_rule(second, &DASHED_RULE) && !tip && !second.ends_with('?') {
        return Some(short(second));
    }
    None
}

/// A hash of what the prompt says, sent in a push instead of the text when
/// `alert_prompt_text` is off. The app hashes its own parse of the screen the
/// same way (`promptFingerprint` in `src/protocol/prompt.ts`) and answers only
/// on an exact match.
///
/// SHA-256 of the UTF-8 lines `q:<question>`, `d:<detail, or empty>`, then
/// `<key>:<label>` for each option in screen order, joined with `\n` (no
/// trailing newline); the first 16 bytes as lowercase hex. The cursor
/// position is left out so moving it does not block an answer.
pub(crate) fn prompt_fingerprint(prompt: &Prompt) -> String {
    let mut lines = vec![
        format!("q:{}", prompt.question),
        format!("d:{}", prompt.detail.as_deref().unwrap_or_default()),
    ];
    lines.extend(
        prompt
            .options
            .iter()
            .map(|option| format!("{}:{}", option.key, option.label)),
    );
    let digest = Sha256::digest(lines.join("\n").as_bytes());
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The fixed answer-button sets the phone app registers as notification
/// categories. Expo warns against `:` and `-` in category ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Category {
    /// Yes / Yes, don't ask again / No / Open.
    YesAlwaysNo,
    /// Yes / No / Open.
    YesNo,
}

impl Category {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::YesAlwaysNo => "prompt_yes_always_no",
            Self::YesNo => "prompt_yes_no",
        }
    }
}

/// A prompt whose options map cleanly onto one category's buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptActions {
    pub category: Category,
    pub yes: PromptOption,
    pub always: Option<PromptOption>,
    pub no: PromptOption,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Yes,
    Always,
    No,
}

fn kind(label: &str) -> Option<Kind> {
    let lower = label.to_lowercase().replace('’', "'");
    if lower == "yes" || lower == "yes, proceed" {
        return Some(Kind::Yes);
    }
    if lower.starts_with("yes, and don't ask again") {
        return Some(Kind::Always);
    }
    // "No" as a word: "No", "No, and tell Claude…", not "Nope".
    let after_no = lower.strip_prefix("no")?;
    if after_no
        .chars()
        .next()
        .is_none_or(|c| !c.is_alphanumeric() && c != '_')
    {
        return Some(Kind::No);
    }
    None
}

/// Maps the options to answer buttons: Yes is the plain "Yes" (or Codex's
/// "Yes, proceed"), Always is "Yes, and don't ask again…", No is the option
/// starting with the word "No". Each must appear at most once, and Yes and No
/// must both be there; otherwise `None` (a plain alert that opens the pane).
/// Other options ("Yes, allow all edits during this session") get no button.
pub(crate) fn prompt_actions(prompt: &Prompt) -> Option<PromptActions> {
    let pick = |want: Kind| -> Result<Option<PromptOption>, ()> {
        let mut matches = prompt
            .options
            .iter()
            .filter(|option| kind(&option.label) == Some(want));
        let first = matches.next().cloned();
        if matches.next().is_some() {
            return Err(());
        }
        Ok(first)
    };
    let yes = pick(Kind::Yes).ok()??;
    let no = pick(Kind::No).ok()??;
    let always = pick(Kind::Always).ok()?;
    Some(PromptActions {
        category: if always.is_some() {
            Category::YesAlwaysNo
        } else {
            Category::YesNo
        },
        yes,
        always,
        no,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/prompts")
                .join(name),
        )
        .unwrap()
    }

    fn opts(labels: &[&str]) -> Vec<PromptOption> {
        labels
            .iter()
            .enumerate()
            .map(|(i, label)| PromptOption {
                key: (i + 1).to_string(),
                label: (*label).to_owned(),
            })
            .collect()
    }

    fn labels(prompt: &Prompt) -> Vec<&str> {
        prompt.options.iter().map(|o| o.label.as_str()).collect()
    }

    #[test]
    fn reads_a_claude_code_bash_prompt() {
        assert_eq!(
            parse_prompt(&fixture("claude-bash.txt")),
            Some(Prompt {
                question: "Do you want to proceed?".to_owned(),
                detail: Some("npm test 2>&1 | tail -100".to_owned()),
                options: opts(&[
                    "Yes",
                    "Yes, and don’t ask again for: npm test *",
                    "Yes, and switch to auto mode · auto mode handles these prompts for you",
                    "No",
                ]),
                selected: "1".to_owned(),
            })
        );
    }

    #[test]
    fn reads_a_boxed_claude_code_bash_prompt_with_a_wrapped_option() {
        assert_eq!(
            parse_prompt(&fixture("claude-bash-boxed.txt")),
            Some(Prompt {
                question: "Do you want to proceed?".to_owned(),
                detail: Some("npx playwright test --project=chromium".to_owned()),
                options: opts(&[
                    "Yes",
                    "Yes, and don't ask again for npx playwright test commands in ~/src/storefront",
                    "Yes, allow all Bash commands during this session",
                    "No, and tell Claude what to do differently",
                ]),
                selected: "1".to_owned(),
            })
        );
    }

    #[test]
    fn reads_a_claude_code_edit_prompt() {
        assert_eq!(
            parse_prompt(&fixture("claude-edit.txt")),
            Some(Prompt {
                question: "Do you want to make this edit to Cart.tsx?".to_owned(),
                detail: Some("src/components/Cart.tsx".to_owned()),
                options: opts(&[
                    "Yes",
                    "Yes, allow all edits during this session",
                    "No, and tell Claude what to do differently",
                ]),
                selected: "1".to_owned(),
            })
        );
    }

    #[test]
    fn reads_a_codex_approval_prompt() {
        assert_eq!(
            parse_prompt(&fixture("codex-allow.txt")),
            Some(Prompt {
                question: "Allow command?".to_owned(),
                detail: Some("npm test -- --run".to_owned()),
                options: opts(&[
                    "Yes, proceed",
                    "Yes, and don't ask again for this command",
                    "No, and tell Codex what to do differently",
                ]),
                selected: "1".to_owned(),
            })
        );
    }

    #[test]
    fn reads_a_codex_run_prompt_with_key_hints() {
        let prompt = parse_prompt(&fixture("codex-run.txt")).unwrap();
        assert_eq!(
            prompt.question,
            "Would you like to run the following command?"
        );
        assert_eq!(
            prompt.detail.as_deref(),
            Some("npx prisma migrate diff --from-schema-datasource prisma/schema.prisma")
        );
        assert_eq!(
            labels(&prompt),
            vec![
                "Yes, proceed",
                "Yes, and don't ask again for this command",
                "No, and tell Codex what to do differently",
            ]
        );
    }

    #[test]
    fn tracks_the_selected_option() {
        let text = fixture("claude-bash.txt")
            .replace(" ❯ 1. Yes", "   1. Yes")
            .replace("   2. Yes, and", " ❯ 2. Yes, and");
        assert_eq!(parse_prompt(&text).unwrap().selected, "2");
    }

    #[test]
    fn ignores_numbered_lists_that_are_not_prompts() {
        assert_eq!(parse_prompt(&fixture("list-idle.txt")), None);
        assert_eq!(parse_prompt(&fixture("list-question.txt")), None);
        assert_eq!(parse_prompt(&fixture("working.txt")), None);
        assert_eq!(parse_prompt(""), None);
    }

    #[test]
    fn needs_a_question_line() {
        let text = fixture("claude-bash.txt").replace("Do you want to proceed?", "Pick one");
        assert_eq!(parse_prompt(&text), None);
    }

    #[test]
    fn needs_options_numbered_from_one_with_no_gaps() {
        let text = fixture("claude-bash.txt").replace("   2. Yes,", "   5. Yes,");
        assert_eq!(parse_prompt(&text), None);
        assert_eq!(parse_prompt("Continue?\n❯ 1. Yes"), None);
    }

    #[test]
    fn needs_the_options_at_the_bottom_of_the_screen() {
        let text = format!(
            "{}\nmore\noutput\nafter\nthe\nprompt\n",
            fixture("claude-bash.txt")
        );
        assert_eq!(parse_prompt(&text), None);
    }

    /// The app's prompt tests assert the same value for the same fixture, so
    /// the two canonical forms cannot drift apart.
    #[test]
    fn fingerprints_the_question_detail_and_options() {
        let prompt = parse_prompt(&fixture("claude-bash.txt")).unwrap();
        assert_eq!(
            prompt_fingerprint(&prompt),
            "36dfc8675e9d416e241ddc54a4bb19d3"
        );
        let moved = Prompt {
            selected: "3".to_owned(),
            ..prompt.clone()
        };
        assert_eq!(prompt_fingerprint(&moved), prompt_fingerprint(&prompt));
        let mut other = prompt.clone();
        other.detail = Some("rm -rf build".to_owned());
        assert_ne!(prompt_fingerprint(&other), prompt_fingerprint(&prompt));
        let mut relabeled = prompt.clone();
        relabeled.options[3].label = "Yes, always".to_owned();
        assert_ne!(prompt_fingerprint(&relabeled), prompt_fingerprint(&prompt));
        let mut no_detail = prompt;
        no_detail.detail = None;
        let mut empty_detail = no_detail.clone();
        empty_detail.detail = Some(String::new());
        assert_eq!(
            prompt_fingerprint(&no_detail),
            prompt_fingerprint(&empty_detail)
        );
    }

    #[test]
    fn strips_only_known_key_hints() {
        assert_eq!(option_label("No (esc)"), "No");
        assert_eq!(option_label("Yes  (ctrl+y)"), "Yes");
        assert_eq!(option_label("Go (ctrl+yy)"), "Go (ctrl+yy)");
        assert_eq!(option_label("Run it (twice)"), "Run it (twice)");
        assert_eq!(option_label("(y)"), "(y)");
    }

    #[test]
    fn maps_options_to_buttons() {
        let actions = |name: &str| prompt_actions(&parse_prompt(&fixture(name)).unwrap());
        let bash = actions("claude-bash.txt").unwrap();
        assert_eq!(bash.category, Category::YesAlwaysNo);
        assert_eq!(bash.yes.key, "1");
        assert_eq!(bash.always.as_ref().map(|o| o.key.as_str()), Some("2"));
        assert_eq!(bash.no.key, "4");

        let boxed = actions("claude-bash-boxed.txt").unwrap();
        assert_eq!(boxed.category, Category::YesAlwaysNo);
        assert_eq!(boxed.no.label, "No, and tell Claude what to do differently");

        // "Yes, allow all edits during this session" gets no button.
        let edit = actions("claude-edit.txt").unwrap();
        assert_eq!(edit.category, Category::YesNo);
        assert_eq!((edit.yes.key.as_str(), edit.no.key.as_str()), ("1", "3"));
        assert_eq!(edit.always, None);

        let codex = actions("codex-allow.txt").unwrap();
        assert_eq!(codex.category, Category::YesAlwaysNo);
        assert_eq!(codex.yes.label, "Yes, proceed");
        assert_eq!(Category::YesNo.id(), "prompt_yes_no");
        assert_eq!(Category::YesAlwaysNo.id(), "prompt_yes_always_no");
    }

    #[test]
    fn unclear_options_get_no_buttons() {
        let prompt = |labels: &[&str]| Prompt {
            question: "Continue?".to_owned(),
            detail: None,
            options: opts(labels),
            selected: "1".to_owned(),
        };
        // No plain No.
        assert_eq!(prompt_actions(&prompt(&["Yes", "Nope"])), None);
        // No plain Yes.
        assert_eq!(prompt_actions(&prompt(&["Yes, overwrite", "No"])), None);
        // Two of a kind.
        assert_eq!(prompt_actions(&prompt(&["Yes", "Yes", "No"])), None);
        assert_eq!(
            prompt_actions(&prompt(&["Yes", "No", "No, and stop"])),
            None
        );
        assert_eq!(
            prompt_actions(&prompt(&[
                "Yes",
                "Yes, and don't ask again for a",
                "Yes, and don't ask again for b",
                "No",
            ])),
            None
        );
        assert!(prompt_actions(&prompt(&["Yes", "No"])).is_some());
    }
}
