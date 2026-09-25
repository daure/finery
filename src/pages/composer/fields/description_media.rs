use std::time::Instant;

use crate::store::composer::{TicketAttachment, description_media::DescriptionPresentation};
use tuicore::{MouseButton, MouseEventKind};

use super::*;

#[derive(Default)]
pub(super) struct DescriptionMedia {
    pub selected: DescriptionPresentation,
    pub source: DescriptionPresentation,
    pub changes: DescriptionPresentation,
    context: Option<(String, ComposerViewMode)>,
    last_click: Option<(Instant, String, u16, u16)>,
}

fn diff_tickets(state: &ComposerState) -> (Option<&Ticket>, Option<&Ticket>) {
    if let Some(snapshot) = state
        .selected_change()
        .and_then(|change| change.submitted.as_ref())
    {
        (snapshot.original.as_ref(), snapshot.updated.as_ref())
    } else {
        (state.selected_source(), state.selected_changes())
    }
}

impl DescriptionMedia {
    pub fn sync(&mut self, state: &ComposerState) -> bool {
        let selected = DescriptionPresentation::for_ticket(state.selected_ticket());
        let (source, changes) = diff_tickets(state);
        let source = DescriptionPresentation::for_ticket(source);
        let changes = DescriptionPresentation::for_ticket(changes);
        let context = state
            .selected_ticket()
            .map(|ticket| (ticket.key.clone(), state.view_mode));
        let changed = self.selected != selected
            || self.source != source
            || self.changes != changes
            || self.context != context;
        if changed {
            self.selected = selected;
            self.source = source;
            self.changes = changes;
            self.context = context;
            self.last_click = None;
        }
        changed
    }
}

impl BoundDescription {
    fn image_reference_at(&self, column: u16, row: u16) -> Option<(String, TicketAttachment)> {
        let state = self.state.borrow();
        let (presentation, ticket, character, side) = if state.view_mode == ComposerViewMode::Diff {
            let (location, character_column) = self.diff.text_position_at(column, row)?;
            let (source, changes) = diff_tickets(&state);
            if let Some(line) = location.old_line {
                (
                    &self.media.source,
                    source?,
                    self.media.source.character_at(line, character_column)?,
                    "source",
                )
            } else {
                (
                    &self.media.changes,
                    changes?,
                    self.media
                        .changes
                        .character_at(location.new_line?, character_column)?,
                    "changes",
                )
            }
        } else if self.shows_media_preview() {
            (
                &self.media.selected,
                state.selected_ticket()?,
                self.preview.text_index_at(column, row)?,
                "selected",
            )
        } else {
            return None;
        };
        let id = presentation.attachment_at(character)?;
        let attachment = ticket
            .attachments
            .iter()
            .find(|attachment| attachment.id == id)?
            .clone();
        Some((format!("{side}:{id}"), attachment))
    }

    pub(super) fn handle_image_reference(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        let TuiEvent::Mouse(mouse) = event else {
            self.media.last_click = None;
            return false;
        };
        if matches!(mouse.kind, MouseEventKind::Up(_) | MouseEventKind::Moved) {
            return false;
        }
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            self.media.last_click = None;
            return false;
        }
        let Some((identity, attachment)) = self.image_reference_at(mouse.column, mouse.row) else {
            self.media.last_click = None;
            return false;
        };
        let now = Instant::now();
        let double = self
            .media
            .last_click
            .take()
            .is_some_and(|(time, previous, x, y)| {
                previous == identity
                    && now.duration_since(time) <= Duration::from_millis(500)
                    && x.abs_diff(mouse.column) <= 1
                    && y.abs_diff(mouse.row) <= 1
            });
        if double {
            self.description_actions
                .borrow_mut()
                .push(DescriptionAction::OpenImage(attachment));
        } else {
            self.media.last_click = Some((now, identity, mouse.column, mouse.row));
        }
        ctx.stop_propagation();
        true
    }
}

#[cfg(test)]
#[path = "../tests/description_media.rs"]
mod tests;
