use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
};

use tuicore::EventCtx;

use crate::{
    jira::{SubmitBatchOutcome, TicketSubmitOutcome},
    service::AppService,
    store::composer::{ComposerAction, ComposerState, TicketChange},
};

pub(super) struct SubmissionController {
    state: Rc<RefCell<ComposerState>>,
    service: AppService,
    sender: Sender<Result<SubmitBatchOutcome, String>>,
    receiver: Receiver<Result<SubmitBatchOutcome, String>>,
    submitting: bool,
    notices: Vec<(bool, String, String)>,
}

impl SubmissionController {
    pub(super) fn new(state: Rc<RefCell<ComposerState>>, service: AppService) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            state,
            service,
            sender,
            receiver,
            submitting: false,
            notices: Vec::new(),
        }
    }

    pub(super) fn is_submitting(&self) -> bool {
        self.submitting
    }

    pub(super) fn start(&mut self, changes: Vec<TicketChange>) {
        if self.submitting || changes.is_empty() {
            return;
        }
        self.submitting = true;
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-submit".into())
            .spawn(move || {
                let _ = sender.send(service.submit_ticket_changes(&changes));
            })
        {
            self.submitting = false;
            self.notices.push((
                true,
                "Submit failed".into(),
                format!("Could not start Jira submit: {error}"),
            ));
        }
    }

    pub(super) fn drain_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.receiver.try_recv() {
            self.submitting = false;
            changed = true;
            match result {
                Err(error) => self
                    .notices
                    .push((true, "Submit failed".into(), error)),
                Ok(SubmitBatchOutcome::Conflict(keys)) => self.notices.push((
                    false,
                    "Submit cancelled".into(),
                    format!(
                        "Jira changed since this change set was composed: {}. Refresh those tickets before retrying.",
                        keys.join(", ")
                    ),
                )),
                Ok(SubmitBatchOutcome::Completed(outcomes)) => self.apply_outcomes(outcomes),
            }
        }
        changed
    }

    pub(super) fn drain_notices(&mut self, ctx: &mut EventCtx<()>) {
        for (error, title, message) in self.notices.drain(..) {
            if error {
                ctx.notify(tuicore::Notification::error(title, message));
            } else {
                ctx.notify(tuicore::Notification::warning(title, message));
            }
        }
    }

    fn apply_outcomes(&mut self, outcomes: Vec<TicketSubmitOutcome>) {
        let active_id = self.state.borrow().active_change_set.clone();
        for outcome in outcomes {
            match outcome.result {
                Ok(snapshot) => {
                    self.state
                        .borrow_mut()
                        .dispatch(ComposerAction::CompleteSubmission {
                            id: outcome.id,
                            snapshot,
                        })
                }
                Err(failure) => {
                    if let Some(refresh) = failure.refresh {
                        let (original, updated) = *refresh;
                        self.state.borrow_mut().dispatch(
                            ComposerAction::RefreshAfterFailedSubmission {
                                id: outcome.id.clone(),
                                original,
                                updated,
                            },
                        );
                    }
                    self.notices
                        .push((true, format!("{} failed", outcome.id), failure.message));
                }
            }
        }
        if let Some(set) = active_id.and_then(|id| {
            self.state
                .borrow()
                .change_sets
                .iter()
                .find(|set| set.id == id)
                .cloned()
        }) {
            self.service.save_change_set(set);
        }
    }
}
