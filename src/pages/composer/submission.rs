use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
};

use crate::{
    jira::{SubmitBatchOutcome, TicketSubmitOutcome},
    service::AppService,
    store::composer::{ComposerAction, ComposerState, TicketChange},
};

pub(super) struct SubmissionController {
    state: Rc<RefCell<ComposerState>>,
    service: AppService,
    sender: Sender<(u64, String, Result<SubmitBatchOutcome, String>)>,
    receiver: Receiver<(u64, String, Result<SubmitBatchOutcome, String>)>,
    request_id: u64,
    active_request: Option<(u64, String, Vec<String>)>,
    submitting: bool,
}

impl SubmissionController {
    pub(super) fn new(state: Rc<RefCell<ComposerState>>, service: AppService) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            state,
            service,
            sender,
            receiver,
            request_id: 0,
            active_request: None,
            submitting: false,
        }
    }

    pub(super) fn is_submitting(&self) -> bool {
        self.submitting
    }

    pub(super) fn start(&mut self, changes: Vec<TicketChange>, ctx: &mut tuicore::EventCtx<()>) {
        if self.submitting || changes.is_empty() {
            return;
        }
        let Some(change_set_id) = self.state.borrow().active_change_set.clone() else {
            self.service
                .report_notification(tuicore::Notification::error(
                    "Commit failed",
                    "Open a change set before committing",
                ));
            return;
        };
        if !self.state.borrow_mut().begin_submission(&change_set_id) {
            self.service
                .report_notification(tuicore::Notification::error(
                    "Commit failed",
                    "The change set is unavailable",
                ));
            return;
        }
        let create_attempts = changes
            .iter()
            .filter(|change| change.kind == crate::store::composer::ChangeKind::Added)
            .map(|change| change.id.clone())
            .collect::<Vec<_>>();
        let persisted_set = (!create_attempts.is_empty()).then(|| {
            self.state
                .borrow_mut()
                .dispatch(ComposerAction::MarkCreateAttempts {
                    change_set_id: change_set_id.clone(),
                    ids: create_attempts.clone(),
                })
                .expect("submission target must exist");
            self.state
                .borrow()
                .change_sets
                .iter()
                .find(|set| set.id == change_set_id)
                .cloned()
                .expect("submission target must exist")
        });
        self.submitting = true;
        self.request_id = self.request_id.saturating_add(1);
        let request_id = self.request_id;
        self.active_request = Some((request_id, change_set_id.clone(), create_attempts.clone()));
        ctx.request_tick();
        let service = self.service.clone();
        let sender = self.sender.clone();
        let submitted_change_set_id = change_set_id.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-commit".into())
            .spawn(move || {
                let _ = sender.send((
                    request_id,
                    submitted_change_set_id,
                    persisted_set.map_or_else(
                        || Ok(service.submit_ticket_changes(&changes)),
                        |set| {
                            service
                                .save_change_set_durably(set)
                                .map_err(|error| {
                                    format!("Could not persist Jira create attempt: {error}")
                                })
                                .map(|_| service.submit_ticket_changes(&changes))
                        },
                    ),
                ));
            })
        {
            self.submitting = false;
            self.active_request = None;
            self.state.borrow_mut().end_submission(&change_set_id);
            if let Err(persist_error) =
                self.clear_create_attempts_durably(&change_set_id, &create_attempts)
            {
                self.report_create_attempt_clear_failure(&persist_error);
            }
            self.service
                .report_notification(tuicore::Notification::error(
                    "Commit failed",
                    format!("Could not start Jira commit: {error}"),
                ));
        }
    }

    pub(super) fn drain_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok((request_id, change_set_id, result)) = self.receiver.try_recv() {
            let Some((active_request_id, active_change_set_id, active_create_attempts)) =
                self.active_request.as_ref()
            else {
                continue;
            };
            if (*active_request_id, active_change_set_id.as_str())
                != (request_id, change_set_id.as_str())
            {
                continue;
            }
            let create_attempts = active_create_attempts.clone();
            self.submitting = false;
            self.active_request = None;
            self.state.borrow_mut().end_submission(&change_set_id);
            changed = true;
            match result {
                Err(error) => {
                    if let Err(persist_error) =
                        self.clear_create_attempts_durably(&change_set_id, &create_attempts)
                    {
                        self.report_create_attempt_clear_failure(&persist_error);
                    }
                    self.service
                        .report_notification(tuicore::Notification::error("Commit failed", error));
                }
                Ok(SubmitBatchOutcome::PreflightError(error)) => {
                    if let Err(persist_error) =
                        self.clear_create_attempts_durably(&change_set_id, &create_attempts)
                    {
                        self.report_create_attempt_clear_failure(&persist_error);
                    }
                    self.service
                        .report_notification(tuicore::Notification::error("Commit failed", error));
                }
                Ok(SubmitBatchOutcome::Conflict(keys)) => {
                    if let Err(persist_error) =
                        self.clear_create_attempts_durably(&change_set_id, &create_attempts)
                    {
                        self.report_create_attempt_clear_failure(&persist_error);
                    }
                    self.service.report_notification(tuicore::Notification::warning(
                        "Commit cancelled",
                        format!(
                            "Jira changed since this change set was composed: {}. Refresh those tickets before retrying.",
                            keys.join(", ")
                        ),
                    ));
                }
                Ok(SubmitBatchOutcome::Completed(outcomes)) => {
                    self.apply_outcomes(&change_set_id, outcomes)
                }
            }
        }
        changed
    }

    fn apply_outcomes(&mut self, change_set_id: &str, outcomes: Vec<TicketSubmitOutcome>) {
        let mut submitted = Vec::new();
        for outcome in outcomes {
            if !self.state.borrow().has_change(change_set_id, &outcome.id) {
                self.service
                    .report_notification(tuicore::Notification::error(
                        "Commit result lost",
                        format!(
                            "Jira returned a result for {} but its change-set target is unavailable",
                            outcome.id
                        ),
                    ));
                continue;
            }
            match outcome.result {
                Ok(snapshot) => {
                    if let Some(ticket) = snapshot.updated.as_ref().or(snapshot.original.as_ref()) {
                        submitted.push((ticket.key.clone(), ticket.title.clone()));
                    }
                    self.state
                        .borrow_mut()
                        .dispatch(ComposerAction::CompleteSubmission {
                            change_set_id: change_set_id.into(),
                            id: outcome.id,
                            snapshot,
                        })
                        .expect("checked submission target must exist");
                }
                Err(failure) => {
                    if failure.retry_blocked {
                        self.state
                            .borrow_mut()
                            .dispatch(ComposerAction::BlockTicketRetry {
                                change_set_id: change_set_id.into(),
                                id: outcome.id.clone(),
                            })
                            .expect("checked submission target must exist");
                    } else if failure.refresh.is_none() {
                        self.state
                            .borrow_mut()
                            .dispatch(ComposerAction::ResolveCreateAttempt {
                                change_set_id: change_set_id.into(),
                                id: outcome.id.clone(),
                            })
                            .expect("checked submission target must exist");
                    }
                    if let Some(refresh) = failure.refresh {
                        let (original, updated) = *refresh;
                        self.state
                            .borrow_mut()
                            .dispatch(ComposerAction::RefreshAfterFailedSubmission {
                                change_set_id: change_set_id.into(),
                                id: outcome.id.clone(),
                                original,
                                updated,
                            })
                            .expect("checked submission target must exist");
                    }
                    self.service
                        .report_notification(tuicore::Notification::error(
                            format!("{} failed", outcome.id),
                            failure.message,
                        ));
                }
            }
        }
        if let Some(set) = self
            .state
            .borrow()
            .change_sets
            .iter()
            .find(|set| set.id == change_set_id)
            .cloned()
        {
            if let Err(error) = self.service.save_change_set_durably(set) {
                self.service
                    .report_notification(tuicore::Notification::error(
                        "Commit persistence failed",
                        format!(
                            "Jira returned a result, but Finery could not save its reconciliation state: {error}"
                        ),
                    ));
            }
        }
        match submitted.as_slice() {
            [] => {}
            [(key, title)] => self
                .service
                .report_notification(tuicore::Notification::success(
                    "Ticket committed",
                    format!("{key} · {title}"),
                )),
            submitted => self
                .service
                .report_notification(tuicore::Notification::success(
                    "Tickets committed",
                    format!("{} tickets committed", submitted.len()),
                )),
        }
    }

    fn clear_create_attempts_durably(
        &mut self,
        change_set_id: &str,
        create_attempts: &[String],
    ) -> Result<(), String> {
        if create_attempts.is_empty() {
            return Ok(());
        }
        for id in create_attempts {
            self.state
                .borrow_mut()
                .dispatch(ComposerAction::ResolveCreateAttempt {
                    change_set_id: change_set_id.into(),
                    id: id.clone(),
                })
                .expect("submission target must exist");
        }
        let set = self
            .state
            .borrow()
            .change_sets
            .iter()
            .find(|set| set.id == change_set_id)
            .cloned()
            .expect("submission target must exist");
        if let Err(error) = self.service.save_change_set_durably(set) {
            self.state
                .borrow_mut()
                .dispatch(ComposerAction::MarkCreateAttempts {
                    change_set_id: change_set_id.into(),
                    ids: create_attempts.to_vec(),
                })
                .expect("submission target must exist");
            return Err(error);
        }
        Ok(())
    }

    fn report_create_attempt_clear_failure(&self, error: &str) {
        self.service
            .report_notification(tuicore::Notification::error(
                "Commit marker persistence failed",
                format!(
                    "Could not clear the Jira create-attempt marker; retry remains blocked: {error}"
                ),
            ));
    }
}
