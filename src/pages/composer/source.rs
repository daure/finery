use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
};

use crate::{
    service::AppService,
    store::composer::{ComposerAction, ComposerState, ComposerViewMode, Ticket, TicketChange},
};

#[derive(Clone, PartialEq, Eq)]
enum SourceRequest {
    Selected(String),
    All,
}

pub(super) struct SourceController {
    state: Rc<RefCell<ComposerState>>,
    service: AppService,
    sender: Sender<(u64, String, Result<Ticket, String>)>,
    receiver: Receiver<(u64, String, Result<Ticket, String>)>,
    generation: u64,
    requested: Option<SourceRequest>,
    loading: usize,
    refresh_count: usize,
    refresh_failures: usize,
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
            loading: 0,
            refresh_count: 0,
            refresh_failures: 0,
            mode: None,
        }
    }

    pub(super) fn ensure_selected(&mut self) {
        if !self.state.borrow().remote_queries_allowed() {
            if self.requested.take().is_some() || self.loading > 0 {
                self.generation = self.generation.saturating_add(1);
            }
            self.loading = 0;
            return;
        }
        let (target, mode_changed) = {
            let state = self.state.borrow();
            (
                state.selected_change().and_then(source_target),
                self.mode != Some(state.view_mode),
            )
        };
        self.mode = Some(self.state.borrow().view_mode);
        let Some((id, key)) = target else {
            self.requested = None;
            self.loading = 0;
            return;
        };
        if matches!(self.requested.as_ref(), Some(SourceRequest::All)) && self.loading > 0 {
            return;
        }
        if !mode_changed && self.requested == Some(SourceRequest::Selected(id.clone())) {
            return;
        }
        self.requested = Some(SourceRequest::Selected(id.clone()));
        self.loading = 1;
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
            self.loading = 0;
            self.service
                .report_error(format!("could not fetch Jira source: {error}"));
        }
    }

    pub(super) fn refresh_all(&mut self) -> usize {
        if !self.state.borrow().remote_queries_allowed() {
            return 0;
        }
        let targets = self
            .state
            .borrow()
            .active_set()
            .into_iter()
            .flat_map(|set| &set.tickets)
            .filter_map(source_target)
            .collect::<Vec<_>>();
        let target_count = targets.len();
        self.generation = self.generation.saturating_add(1);
        self.requested = Some(SourceRequest::All);
        self.loading = target_count;
        self.refresh_count = target_count;
        self.refresh_failures = 0;
        if targets.is_empty() {
            return 0;
        }
        let generation = self.generation;
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("finery-jira-source-refresh-{generation}"))
            .spawn(move || {
                for (id, key) in targets {
                    let _ = sender.send((generation, id, service.fetch_jira(&key)));
                }
            })
        {
            self.loading = 0;
            self.service
                .report_error(format!("could not refresh Jira sources: {error}"));
            return 0;
        }
        target_count
    }

    pub(super) fn is_loading(&self) -> bool {
        self.loading > 0
    }

    pub(super) fn drain(&mut self) -> bool {
        let mut changed = false;
        while let Ok((generation, id, result)) = self.receiver.try_recv() {
            if generation != self.generation {
                continue;
            }
            let refreshing = matches!(self.requested, Some(SourceRequest::All));
            self.loading = self.loading.saturating_sub(1);
            match result {
                Ok(ticket) => self
                    .state
                    .borrow_mut()
                    .dispatch(ComposerAction::SetSource { id, ticket }),
                Err(error) => {
                    if refreshing {
                        self.refresh_failures = self.refresh_failures.saturating_add(1);
                    }
                    self.service
                        .report_error(format!("Jira source refresh failed: {error}"));
                }
            }
            if refreshing && self.loading == 0 {
                self.requested = None;
                if self.refresh_failures == 0 {
                    self.service
                        .report_notification(tuicore::Notification::success(
                            "Refresh complete",
                            format!("{} tickets refreshed", self.refresh_count),
                        ));
                }
            }
            changed = true;
        }
        changed
    }
}

fn source_target(change: &TicketChange) -> Option<(String, String)> {
    let key = change
        .submitted
        .as_ref()
        .and_then(|snapshot| snapshot.updated.as_ref())
        .or(change.updated.as_ref())
        .or(change.original.as_ref())
        .map(|ticket| ticket.key.as_str())
        .unwrap_or(change.id.as_str());
    (!key.starts_with("NEW-")).then(|| (change.id.clone(), key.to_owned()))
}
