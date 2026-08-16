use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
};

use crate::{
    service::AppService,
    store::composer::{ComposerAction, ComposerState, ComposerViewMode, Ticket},
};

pub(super) struct SourceController {
    state: Rc<RefCell<ComposerState>>,
    service: AppService,
    sender: Sender<(u64, String, Result<Ticket, String>)>,
    receiver: Receiver<(u64, String, Result<Ticket, String>)>,
    generation: u64,
    requested: Option<String>,
    loading: bool,
    mode: Option<ComposerViewMode>,
}

impl SourceController {
    pub(super) fn new(state: Rc<RefCell<ComposerState>>, service: AppService) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            state,
            service,
            sender,
            receiver,
            generation: 0,
            requested: None,
            loading: false,
            mode: None,
        }
    }

    pub(super) fn ensure(&mut self, force: bool) {
        if !self.state.borrow().remote_queries_allowed() {
            if self.requested.take().is_some() || self.loading {
                self.generation = self.generation.saturating_add(1);
            }
            self.loading = false;
            return;
        }
        let (target, mode_changed) = {
            let state = self.state.borrow();
            let change = state.selected_change();
            (
                change.and_then(|change| {
                    let key = change
                        .submitted
                        .as_ref()
                        .and_then(|snapshot| snapshot.updated.as_ref())
                        .or(change.updated.as_ref())
                        .or(change.original.as_ref())
                        .map(|ticket| ticket.key.as_str())
                        .unwrap_or(change.id.as_str());
                    (!key.starts_with("NEW-")).then(|| (change.id.clone(), key.to_owned()))
                }),
                self.mode != Some(state.view_mode),
            )
        };
        self.mode = Some(self.state.borrow().view_mode);
        let Some((id, key)) = target else {
            self.requested = None;
            self.loading = false;
            return;
        };
        if !force && !mode_changed && self.requested.as_deref() == Some(id.as_str()) {
            return;
        }
        self.requested = Some(id.clone());
        self.loading = true;
        self.generation = self.generation.saturating_add(1);
        let generation = self.generation;
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("finery-jira-source-{generation}"))
            .spawn(move || {
                let _ = sender.send((generation, id, service.fetch_jira(&key)));
            })
        {
            self.loading = false;
            self.service
                .report_error(format!("could not fetch Jira source: {error}"));
        }
    }

    pub(super) fn is_loading(&self) -> bool {
        self.loading
    }

    pub(super) fn drain(&mut self) -> bool {
        let mut changed = false;
        while let Ok((generation, id, result)) = self.receiver.try_recv() {
            if generation != self.generation {
                continue;
            }
            self.loading = false;
            match result {
                Ok(ticket) => self
                    .state
                    .borrow_mut()
                    .dispatch(ComposerAction::SetSource { id, ticket }),
                Err(error) => self
                    .service
                    .report_error(format!("Jira source refresh failed: {error}")),
            }
            changed = true;
        }
        changed
    }
}
