#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkItem {
    pub key: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub priority: String,
    pub assignee: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sprint {
    pub id: u64,
    pub name: String,
    pub state: String,
    pub work_items: Vec<WorkItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BacklogSnapshot {
    pub board_name: String,
    pub sprints: Vec<Sprint>,
    pub work_items: Vec<WorkItem>,
}
