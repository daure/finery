use std::{cell::RefCell, rc::Rc};

use ratatui::{Frame, layout::Rect, style::Style, text::Line, widgets::Paragraph};
use tuicore::{LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, RenderCtx, TuiNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TitleLevel {
    Bad,
    Okay,
    Good,
    Perfect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TitleCheck {
    pub(super) label: &'static str,
    pub(super) level: TitleLevel,
}

pub(super) fn evaluate_title(value: &str) -> [TitleCheck; 3] {
    let title = value.trim();
    [
        TitleCheck {
            label: "Starts with a verb",
            level: starts_with_verb_level(title),
        },
        TitleCheck {
            label: "No second action detected",
            level: one_action_level(title),
        },
        TitleCheck {
            label: "3-8 words for quick scanning",
            level: word_count_level(title),
        },
    ]
}

pub(super) struct TitleFeedback {
    title: Rc<RefCell<String>>,
}

impl TitleFeedback {
    pub(super) fn new(title: Rc<RefCell<String>>) -> Self {
        Self { title }
    }

    pub(super) fn clear(&self) {
        self.title.borrow_mut().clear();
    }
}

impl TuiNode for TitleFeedback {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let width = evaluate_title(&self.title.borrow())
            .iter()
            .map(|check| check.label.chars().count() as u16 + 10)
            .max()
            .unwrap_or_default();
        LayoutSizeHint::content(width, 3).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, _ctx: &mut LayoutCtx) -> LayoutResult {
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, _ctx: &mut RenderCtx<'a>) {
        let theme = tuicore::theme();
        let lines = evaluate_title(&self.title.borrow()).map(|check| {
            let color = match check.level {
                TitleLevel::Bad => theme.error_fg(),
                TitleLevel::Okay => theme.warning_fg(),
                TitleLevel::Good => theme.success_fg(),
                TitleLevel::Perfect => theme.accent_fg(),
            };
            Line::styled(
                format!(
                    "{} {:<7} {}",
                    level_icon(check.level),
                    level_label(check.level),
                    check.label
                ),
                Style::default().fg(color),
            )
        });
        frame.render_widget(Paragraph::new(lines.to_vec()), area);
    }
}

pub(super) fn format_title(value: &str) -> String {
    let mut tokens = sanitize(value)
        .split_whitespace()
        .map(|value| Token {
            value: value.to_owned(),
            protected: is_protected(value),
            corrected: false,
        })
        .collect::<Vec<_>>();

    protect_delimited_spans(&mut tokens);
    correct_contractions(&mut tokens);
    capitalize_first_plain_token(&mut tokens);
    trim_terminal_prose_punctuation(&mut tokens);
    tokens
        .into_iter()
        .map(|token| token.value)
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug)]
struct Token {
    value: String,
    protected: bool,
    corrected: bool,
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_whitespace() || matches!(character as u32, 0x00..=0x1f | 0x7f..=0x9f) {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn is_protected(token: &str) -> bool {
    token.contains("://")
        || token.to_ascii_lowercase().contains("www.")
        || token.contains(['@', '/', '\\', '_', '`', '{', '}', '[', ']', '='])
        || token.contains("::")
        || token.starts_with("--")
        || is_issue_reference(token)
        || is_version(token)
        || is_dotted_technical_token(token)
        || is_quoted_literal(token)
        || has_internal_mixed_case(token)
        || is_all_uppercase_word(token)
}

fn is_issue_reference(token: &str) -> bool {
    let token = token.trim_end_matches(['.', ',', ';', '!', '?', ':']);
    token.strip_prefix('#').is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    }) || token.rsplit_once('-').is_some_and(|(prefix, number)| {
        prefix.len() >= 2
            && prefix.bytes().all(|byte| byte.is_ascii_uppercase())
            && !number.is_empty()
            && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn is_version(token: &str) -> bool {
    let token = token.trim_end_matches(['.', ',', ';', '!', '?', ':']);
    let token = token
        .strip_prefix('v')
        .or_else(|| token.strip_prefix('V'))
        .unwrap_or(token);
    token.contains('.')
        && token
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn is_dotted_technical_token(token: &str) -> bool {
    let token = token.trim_end_matches(['.', ',', ';', '!', '?', ':']);
    let parts = token.split('.').collect::<Vec<_>>();
    parts.len() >= 2
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

fn is_quoted_literal(token: &str) -> bool {
    let token = token.trim_end_matches(['.', ',', ';']);
    (token.starts_with('"') && token.ends_with('"'))
        || (token.starts_with('\'') && token.ends_with('\''))
}

fn has_internal_mixed_case(token: &str) -> bool {
    let letters = token
        .bytes()
        .filter(|byte| byte.is_ascii_alphabetic())
        .collect::<Vec<_>>();
    let Some((&first, rest)) = letters.split_first() else {
        return false;
    };
    !(first.is_ascii_lowercase() && rest.iter().all(|byte| byte.is_ascii_lowercase())
        || first.is_ascii_uppercase() && rest.iter().all(|byte| byte.is_ascii_uppercase())
        || first.is_ascii_uppercase() && rest.iter().all(|byte| byte.is_ascii_lowercase()))
}

fn is_all_uppercase_word(token: &str) -> bool {
    let letters = token.bytes().filter(|byte| byte.is_ascii_alphabetic());
    letters.clone().count() >= 2 && letters.into_iter().all(|byte| byte.is_ascii_uppercase())
}

fn protect_delimited_spans(tokens: &mut [Token]) {
    let mut delimiter = None;
    for token in tokens {
        if let Some(open) = delimiter {
            token.protected = true;
            if ends_with_delimiter(&token.value, open) {
                delimiter = None;
            }
            continue;
        }
        let Some(open) = token
            .value
            .chars()
            .next()
            .filter(|character| matches!(character, '"' | '\'' | '`'))
        else {
            continue;
        };
        token.protected = true;
        if !ends_with_delimiter(&token.value[open.len_utf8()..], open) {
            delimiter = Some(open);
        }
    }
}

fn ends_with_delimiter(token: &str, delimiter: char) -> bool {
    token
        .trim_end_matches(['.', ',', ';', '!', '?', ':'])
        .ends_with(delimiter)
}

fn correct_contractions(tokens: &mut [Token]) {
    for index in 0..tokens.len() {
        if tokens[index].protected {
            continue;
        }
        let next_is_action = tokens
            .get(index + 1)
            .is_some_and(|token| !token.protected && is_action_verb(&token.value));
        let starts_clause = next_is_action && is_clause_start(tokens, index);
        replace_prose_word(&mut tokens[index], |word| {
            match word.to_ascii_lowercase().as_str() {
                "youre" => Some("you're"),
                "dont" => Some("don't"),
                "isnt" => Some("isn't"),
                "theres" => Some("there's"),
                "cant" if next_is_action => Some("can't"),
                "wont" if next_is_action => Some("won't"),
                "lets" if index == 0 && next_is_action => Some("let's"),
                "ill" if starts_clause => Some("I'll"),
                _ => None,
            }
        });
    }
}

fn is_clause_start(tokens: &[Token], index: usize) -> bool {
    index == 0
        || matches!(
            tokens[index - 1].value.to_ascii_lowercase().as_str(),
            "and" | "then" | "," | ";"
        )
        || tokens[index - 1].value.ends_with([',', ';'])
}

fn replace_prose_word(token: &mut Token, replacement: impl FnOnce(&str) -> Option<&'static str>) {
    let word_end = token
        .value
        .trim_end_matches(['.', ',', '!', '?', ';', ':'])
        .len();
    let word = &token.value[..word_end];
    if !word.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return;
    }
    let Some(replacement) = replacement(word) else {
        return;
    };
    let replacement = if word.as_bytes()[0].is_ascii_uppercase() {
        capitalize(replacement)
    } else {
        replacement.to_owned()
    };
    token.value = format!("{replacement}{}", &token.value[word_end..]);
    token.corrected = true;
}

fn capitalize_first_plain_token(tokens: &mut [Token]) {
    let Some(first) = tokens.first_mut().filter(|token| !token.protected) else {
        return;
    };
    let word_end = first
        .value
        .trim_end_matches(['.', ',', '!', '?', ';', ':'])
        .len();
    let word = &first.value[..word_end];
    if (first.corrected || word.bytes().all(|byte| byte.is_ascii_lowercase()))
        && word.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
    {
        first.value = format!("{}{}", capitalize(word), &first.value[word_end..]);
    }
}

fn capitalize(value: &str) -> String {
    let mut bytes = value.as_bytes().to_vec();
    if let Some(first) = bytes.first_mut() {
        first.make_ascii_uppercase();
    }
    String::from_utf8(bytes).expect("ASCII prose remains valid UTF-8")
}

fn trim_terminal_prose_punctuation(tokens: &mut Vec<Token>) {
    while let Some(last) = tokens.last_mut().filter(|token| !token.protected) {
        last.value = last.value.trim_end_matches(['.', ';', ',']).to_owned();
        if !last.value.is_empty() {
            break;
        }
        tokens.pop();
    }
}

fn level_label(level: TitleLevel) -> &'static str {
    match level {
        TitleLevel::Bad => "Bad",
        TitleLevel::Okay => "Okay",
        TitleLevel::Good => "Good",
        TitleLevel::Perfect => "Perfect",
    }
}

fn level_icon(level: TitleLevel) -> &'static str {
    match level {
        TitleLevel::Bad => "\u{f0a9f}",
        TitleLevel::Okay => "\u{f0aa1}",
        TitleLevel::Good => "\u{f0aa3}",
        TitleLevel::Perfect => "\u{f0aa5}",
    }
}

fn starts_with_verb_level(title: &str) -> TitleLevel {
    if title.is_empty() {
        TitleLevel::Bad
    } else if starts_with_action(title) {
        TitleLevel::Perfect
    } else {
        TitleLevel::Okay
    }
}

fn one_action_level(title: &str) -> TitleLevel {
    if title.is_empty() || has_strong_multiple_action_evidence(title) {
        TitleLevel::Bad
    } else if starts_with_action(title) {
        TitleLevel::Perfect
    } else {
        TitleLevel::Good
    }
}

fn word_count_level(title: &str) -> TitleLevel {
    match title.split_whitespace().count() {
        0 => TitleLevel::Bad,
        1 => TitleLevel::Okay,
        2 => TitleLevel::Good,
        3..=8 => TitleLevel::Perfect,
        9..=10 => TitleLevel::Good,
        11..=12 => TitleLevel::Okay,
        _ => TitleLevel::Bad,
    }
}

fn starts_with_action(title: &str) -> bool {
    let mut words = title.split_whitespace();
    match words.next() {
        Some(first) if is_action_verb(first) => true,
        Some(first)
            if first.eq_ignore_ascii_case("let's") || first.eq_ignore_ascii_case("lets") =>
        {
            words.next().is_some_and(is_action_verb)
        }
        _ => false,
    }
}

fn has_strong_multiple_action_evidence(title: &str) -> bool {
    let words = title.split_whitespace().collect::<Vec<_>>();
    words.windows(2).any(|pair| {
        (matches!(normalized_word(pair[0]).as_str(), "and" | "then")
            || matches!(pair[0], "/" | "&" | "+"))
            && is_unambiguous_second_action(pair[1])
    }) || title
        .split([',', ';'])
        .skip(1)
        .filter_map(|clause| clause.split_whitespace().next())
        .any(is_unambiguous_second_action)
}

fn is_unambiguous_second_action(word: &str) -> bool {
    is_action_verb(word)
        && !matches!(
            normalized_word(word).as_str(),
            "account" | "design" | "plan" | "report" | "research" | "review" | "support"
        )
}

fn is_action_verb(word: &str) -> bool {
    matches!(
        normalized_word(word).as_str(),
        "add"
            | "analyze"
            | "audit"
            | "automate"
            | "benchmark"
            | "build"
            | "call"
            | "compare"
            | "configure"
            | "create"
            | "debug"
            | "define"
            | "deploy"
            | "design"
            | "document"
            | "draft"
            | "email"
            | "estimate"
            | "evaluate"
            | "fix"
            | "implement"
            | "integrate"
            | "investigate"
            | "migrate"
            | "model"
            | "optimize"
            | "patch"
            | "plan"
            | "profile"
            | "prototype"
            | "refactor"
            | "release"
            | "research"
            | "review"
            | "schedule"
            | "scope"
            | "specify"
            | "test"
            | "triage"
            | "upgrade"
            | "update"
            | "validate"
            | "verify"
            | "write"
    )
}

fn normalized_word(word: &str) -> String {
    word.trim_matches(|character: char| !character.is_alphabetic())
        .to_ascii_lowercase()
}
