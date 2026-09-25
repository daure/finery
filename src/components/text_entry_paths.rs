use std::collections::HashSet;

use tuicore::{LayoutCtx, TreePath};

#[derive(Default)]
pub(crate) struct TextEntryPaths {
    paths: HashSet<TreePath>,
}

impl TextEntryPaths {
    pub(crate) fn capture(&mut self, ctx: &LayoutCtx, root: &TreePath) {
        self.paths = ctx
            .focus_targets()
            .iter()
            .filter(|target| target.enabled && target.suppress_global_hotkeys)
            .filter_map(|target| target.path.keys().strip_prefix(root.keys()))
            .map(|keys| TreePath::from_keys(keys.iter().cloned()))
            .collect();
    }

    pub(crate) fn contains(&self, path: &TreePath) -> bool {
        self.paths.contains(path)
    }
}
