use serde::{Deserialize, Serialize};

// Optional-field selections use "" for missing values; an empty vector matches every value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct BacklogFilterCriteria {
    pub(crate) estimated: bool,
    pub(crate) issue_types: Vec<String>,
    pub(crate) users: Vec<String>,
    pub(crate) statuses: Vec<String>,
    pub(crate) epics: Vec<String>,
    pub(crate) labels: Vec<String>,
    pub(crate) releases: Vec<String>,
}

pub(crate) fn matches_optional_values<'a>(
    selected: &[String],
    values: impl IntoIterator<Item = &'a str>,
) -> bool {
    if selected.is_empty() {
        return true;
    }
    let mut has_value = false;
    for value in values
        .into_iter()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        has_value = true;
        if selected
            .iter()
            .any(|selection| value.eq_ignore_ascii_case(selection))
        {
            return true;
        }
    }
    !has_value && selected.iter().any(String::is_empty)
}

impl BacklogFilterCriteria {
    pub(crate) fn unfiltered() -> Self {
        Self::default()
    }
}

impl Default for BacklogFilterCriteria {
    fn default() -> Self {
        Self {
            estimated: true,
            issue_types: Vec::new(),
            users: Vec::new(),
            statuses: Vec::new(),
            epics: Vec::new(),
            labels: Vec::new(),
            releases: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SavedBacklogFilter {
    pub(crate) id: u64,
    pub(crate) name: String,
    #[serde(flatten)]
    pub(crate) criteria: BacklogFilterCriteria,
}

impl SavedBacklogFilter {
    pub(crate) fn new(id: u64, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            criteria: BacklogFilterCriteria::unfiltered(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BacklogFilterOptions {
    pub(crate) issue_types: Vec<String>,
    pub(crate) users: Vec<String>,
    pub(crate) statuses: Vec<String>,
    pub(crate) epics: Vec<String>,
    pub(crate) labels: Vec<String>,
    pub(crate) releases: Vec<String>,
}
