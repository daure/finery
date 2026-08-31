use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
};

use crate::{
    service::{AppService, ComposerSourceTicket},
    store::composer::{ComposerAction, ComposerState, ComposerViewMode, TicketChange},
};

#[derive(Clone, PartialEq, Eq)]
enum SourceRequest {
    Selected { change_set_id: String, id: String },
    All(String),
}

enum SourceResponse {
    Selected(u64, String, String, Result<ComposerSourceTicket, String>),
    Refresh(
        u64,
        String,
        Vec<(String, Result<ComposerSourceTicket, String>)>,
    ),
}

pub(super) struct SourceController {
    state: Rc<RefCell<ComposerState>>,
    service: AppService,
    sender: Sender<SourceResponse>,
    receiver: Receiver<SourceResponse>,
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
        let (change_set_id, target, mode_changed) = {
            let state = self.state.borrow();
            (
                state.active_change_set.clone(),
                state.selected_change().and_then(source_target),
                self.mode != Some(state.view_mode),
            )
        };
        self.mode = Some(self.state.borrow().view_mode);
        let Some((change_set_id, (id, key))) = change_set_id.zip(target) else {
            if self.requested.take().is_some() {
                self.generation = self.generation.saturating_add(1);
            }
            self.loading = 0;
            return;
        };
        if let Some(SourceRequest::All(request_change_set_id)) = self.requested.as_ref()
            && self.loading > 0
        {
            if request_change_set_id == &change_set_id {
                return;
            }
            self.generation = self.generation.saturating_add(1);
            self.loading = 0;
        }
        let request = SourceRequest::Selected {
            change_set_id: change_set_id.clone(),
            id: id.clone(),
        };
        if !mode_changed && self.requested.as_ref() == Some(&request) {
            return;
        }
        self.requested = Some(request);
        self.loading = 1;
        self.generation = self.generation.saturating_add(1);
        let generation = self.generation;
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("finery-jira-source-{generation}"))
            .spawn(move || {
                let result = (|| {
                    let mut sources = service.fetch_jira_for_composer(&[key.clone()])?;
                    sources
                        .remove(&key)
                        .ok_or_else(|| format!("Jira did not return requested ticket: {key}"))
                })();
                let _ = sender.send(SourceResponse::Selected(
                    generation,
                    change_set_id,
                    id,
                    result,
                ));
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
        let state = self.state.borrow();
        let Some(change_set_id) = state.active_change_set.clone() else {
            return 0;
        };
        let targets = state
            .active_set()
            .into_iter()
            .flat_map(|set| &set.tickets)
            .filter_map(source_target)
            .collect::<Vec<_>>();
        drop(state);
        let target_count = targets.len();
        self.generation = self.generation.saturating_add(1);
        self.requested = Some(SourceRequest::All(change_set_id.clone()));
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
                let keys = targets
                    .iter()
                    .map(|(_, key)| key.clone())
                    .collect::<Vec<_>>();
                let results = match service.fetch_jira_for_composer(&keys) {
                    Ok(mut sources) => targets
                        .into_iter()
                        .map(|(id, key)| {
                            let source = sources.remove(&key).ok_or_else(|| {
                                format!("Jira did not return requested ticket: {key}")
                            });
                            (id, source)
                        })
                        .collect(),
                    Err(error) => targets
                        .into_iter()
                        .map(|(id, _)| (id, Err(error.clone())))
                        .collect(),
                };
                let _ = sender.send(SourceResponse::Refresh(generation, change_set_id, results));
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
        while let Ok(response) = self.receiver.try_recv() {
            let (generation, change_set_id, responses, refreshing) = match response {
                SourceResponse::Selected(generation, change_set_id, id, result) => {
                    (generation, change_set_id, vec![(id, result)], false)
                }
                SourceResponse::Refresh(generation, change_set_id, responses) => {
                    (generation, change_set_id, responses, true)
                }
            };
            if generation != self.generation {
                continue;
            }
            let response_id = responses.first().map(|(id, _)| id.as_str());
            let Some(request_refreshing) = self.accepts_response(&change_set_id, response_id)
            else {
                continue;
            };
            if refreshing != request_refreshing {
                continue;
            }
            self.loading = if refreshing {
                0
            } else {
                self.loading.saturating_sub(1)
            };
            let failures = responses
                .iter()
                .filter_map(|(_, result)| result.as_ref().err())
                .cloned()
                .collect::<Vec<_>>();
            if failures.is_empty() {
                for (id, result) in responses {
                    let Ok(source) = result else {
                        unreachable!();
                    };
                    let _ = self.state.borrow_mut().dispatch(ComposerAction::SetSource {
                        change_set_id: change_set_id.clone(),
                        id: id.clone(),
                        ticket: source.ticket,
                    });
                    let _ = self
                        .state
                        .borrow_mut()
                        .dispatch(ComposerAction::SetPresentation {
                            change_set_id: change_set_id.clone(),
                            id,
                            presentation: source.presentation,
                        });
                }
                if refreshing && let Some(set) = self.state.borrow().active_set().cloned() {
                    self.service.save_change_set(set);
                }
            } else {
                if refreshing {
                    self.refresh_failures = self.refresh_failures.saturating_add(failures.len());
                }
                self.service.report_error(format!(
                    "Jira source refresh failed: {}",
                    failures.join("; ")
                ));
            }
            if refreshing && self.loading == 0 {
                self.requested = None;
            }
            changed = true;
        }
        changed
    }

    fn accepts_response(&self, change_set_id: &str, response_id: Option<&str>) -> Option<bool> {
        let state = self.state.borrow();
        if !state.remote_queries_allowed()
            || state.active_change_set.as_deref() != Some(change_set_id)
        {
            return None;
        }
        match self.requested.as_ref()? {
            SourceRequest::Selected {
                change_set_id: requested_change_set_id,
                id: requested_id,
            } => (requested_change_set_id == change_set_id && response_id == Some(requested_id))
                .then_some(false),
            SourceRequest::All(requested_change_set_id) => {
                (requested_change_set_id == change_set_id).then_some(true)
            }
        }
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
