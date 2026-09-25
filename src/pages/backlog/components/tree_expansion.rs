use std::collections::HashSet;

use tuicore::{EventCtx, TuiEvent};

use super::{BacklogRowContent, BacklogTree};
use crate::components::work_item_rows::WorkItemKind;

impl BacklogTree {
    pub(super) fn handle_bulk_expansion(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };
        let view = self.control.data_view();
        if !view.is_focused()
            || view.is_searching()
            || self.control.is_reordering()
            || !tuicore::keybindings()
                .data_view()
                .toggle_all_expansion_matches(*key)
        {
            return false;
        }

        let expanded: HashSet<_> = self
            .control
            .items()
            .iter()
            .filter(|row| {
                !matches!(&row.content, BacklogRowContent::WorkItem(item) if matches!(item.item.kind, WorkItemKind::Subtask))
            })
            .filter_map(|row| row.parent_id.clone())
            .collect();
        if expanded.is_subset(&view.tree_expansion_snapshot()) {
            self.control.data_view_mut().collapse_all();
        } else {
            let mut highlighted = view.highlighted_id();
            let mut highlight_candidates = Vec::new();
            while let Some(id) = highlighted {
                if highlight_candidates.contains(&id) {
                    break;
                }
                highlighted = self
                    .control
                    .items()
                    .iter()
                    .find(|row| row.id == id)
                    .and_then(|row| row.parent_id.clone());
                highlight_candidates.push(id);
            }
            let view = self.control.data_view_mut();
            view.restore_tree_expansion(expanded);
            for id in highlight_candidates {
                if view.highlight_id(&id).handled {
                    break;
                }
            }
        }
        self.control.data_view_mut().reveal_highlighted_centered();
        ctx.request_layout();
        ctx.request_redraw();
        ctx.stop_propagation();
        true
    }
}
