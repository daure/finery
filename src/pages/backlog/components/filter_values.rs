use ratatui::style::Style;

use crate::store::work_items::saved_filter::SavedBacklogFilter;

pub(super) fn saved_filter_label(filter: &SavedBacklogFilter) -> &str {
    if filter.name.trim().is_empty() {
        "New filter"
    } else {
        &filter.name
    }
}

pub(super) fn sorted_saved_filters(filters: &[SavedBacklogFilter]) -> Vec<SavedBacklogFilter> {
    let mut filters = filters.to_vec();
    filters.sort_by_cached_key(|filter| saved_filter_label(filter).to_lowercase());
    filters
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::pages::backlog) enum SavedFilterField {
    IssueTypes,
    Users,
    Statuses,
    Epics,
    Labels,
    Releases,
}

impl SavedFilterField {
    pub(super) fn hotkey(self) -> &'static str {
        match self {
            Self::IssueTypes => "shift+t",
            Self::Users => "shift+u",
            Self::Statuses => "shift+s",
            Self::Epics => "shift+e",
            Self::Labels => "shift+l",
            Self::Releases => "shift+a",
        }
    }

    fn missing_label(self) -> Option<&'static str> {
        match self {
            Self::Users => Some("Unassigned"),
            Self::Epics => Some("No epic"),
            Self::Labels => Some("No labels"),
            Self::Releases => Some("No fix version"),
            Self::IssueTypes | Self::Statuses => None,
        }
    }

    fn is_missing(self, value: &str) -> bool {
        self.missing_label().is_some()
            && (value.trim().is_empty()
                || (self == Self::Users && value.trim().eq_ignore_ascii_case("Unassigned")))
    }

    pub(super) fn options(self, values: Vec<String>) -> Vec<String> {
        if self.missing_label().is_none() {
            return values;
        }
        std::iter::once(String::new())
            .chain(values.into_iter().filter(|value| !self.is_missing(value)))
            .collect()
    }

    pub(super) fn selection(self, values: Vec<String>) -> Vec<String> {
        values
            .into_iter()
            .map(|value| {
                if self.is_missing(&value) {
                    String::new()
                } else {
                    value
                }
            })
            .collect()
    }

    pub(super) fn label(self, value: &str) -> String {
        if self.is_missing(value) {
            self.missing_label().unwrap_or_default().into()
        } else {
            value.into()
        }
    }

    pub(super) fn sort_values(self, values: &mut [String]) {
        values.sort_by_cached_key(|value| self.label(value).to_lowercase());
    }

    pub(super) fn style(self, value: &str) -> Option<Style> {
        self.is_missing(value)
            .then(|| Style::default().fg(tuicore::theme().muted_fg()))
    }
}

// ListControl reserves an empty option ID for an unfinished required field.
pub(super) fn option_id(value: &str) -> String {
    format!("value:{value}")
}

pub(super) fn option_value(id: &str) -> &str {
    id.strip_prefix("value:").expect("filter option ID")
}
