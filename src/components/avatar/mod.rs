use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use ratatui::{
    style::{Modifier, Style},
    text::Span,
};

const PARTICLES: &[&str] = &[
    "da", "de", "del", "der", "di", "dos", "la", "le", "van", "von",
];

pub(crate) fn bubble_span(name: &str) -> Span<'static> {
    let theme = tuicore::theme();
    if is_unassigned(name) {
        return Span::styled(
            "@--",
            Style::default()
                .fg(theme.muted_fg())
                .add_modifier(Modifier::DIM),
        );
    }
    Span::styled(
        format!("@{}", initials(name)),
        Style::default()
            .fg(avatar_color(name))
            .add_modifier(Modifier::BOLD),
    )
}

pub(crate) fn initials(name: &str) -> String {
    if is_unassigned(name) {
        return "--".into();
    }
    let tokens = name.split_whitespace().collect::<Vec<_>>();
    if tokens.len() == 1 {
        return take_initials(tokens[0], 2);
    }
    let last = tokens
        .iter()
        .rev()
        .find(|token| !is_particle(token))
        .copied()
        .unwrap_or(tokens[tokens.len() - 1]);
    format!(
        "{}{}",
        first_grapheme(tokens[0]).to_uppercase(),
        first_grapheme(last).to_uppercase()
    )
}

fn is_unassigned(name: &str) -> bool {
    name.trim().is_empty() || name == "--" || name.eq_ignore_ascii_case("unassigned")
}

fn avatar_color(name: &str) -> ratatui::style::Color {
    let theme = tuicore::theme();
    let colors = [
        theme.accent_fg(),
        theme.success_fg(),
        theme.warning_fg(),
        theme.error_fg(),
    ];
    colors[hash_index(name) % colors.len()]
}

fn hash_index(name: &str) -> usize {
    let mut hasher = DefaultHasher::new();
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
        .hash(&mut hasher);
    hasher.finish() as usize
}

fn is_particle(token: &str) -> bool {
    PARTICLES.contains(&token.to_ascii_lowercase().as_str())
}

fn take_initials(token: &str, count: usize) -> String {
    token
        .chars()
        .take(count)
        .flat_map(char::to_uppercase)
        .collect()
}

fn first_grapheme(token: &str) -> String {
    token
        .chars()
        .next()
        .map_or_else(|| "?".into(), |ch| ch.into())
}

#[cfg(test)]
mod tests;
