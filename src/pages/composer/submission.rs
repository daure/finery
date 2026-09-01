use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::{self, Receiver, TryRecvError},
};

use crate::{
    service::{AppService, SubmittedComposerChangeSet, composer_service::SubmitChangeSetOutcome},
    store::composer::{ComposerState, TicketChange},
};

pub(super) struct SubmissionController {
    state: Rc<RefCell<ComposerState>>,
    service: AppService,
    response: Option<Receiver<Result<SubmittedComposerChangeSet, String>>>,
    preflight_error: Option<String>,
}

impl SubmissionController {
    pub(super) fn new(state: Rc<RefCell<ComposerState>>, service: AppService) -> Self {
        Self {
            state,
            service,
            response: None,
            preflight_error: None,
        }
    }

    pub(super) fn is_submitting(&self) -> bool {
        self.response.is_some()
    }

    pub(super) fn take_preflight_error(&mut self) -> Option<String> {
        self.preflight_error.take()
    }

    pub(super) fn start(&mut self, changes: Vec<TicketChange>, ctx: &mut tuicore::EventCtx<()>) {
        if self.is_submitting() || changes.is_empty() {
            return;
        }
        let Some(change_set_id) = self.state.borrow().active_change_set.clone() else {
            self.notify_error("Commit failed", "Open a change set before committing");
            return;
        };
        if !self.state.borrow_mut().begin_submission(&change_set_id) {
            self.notify_error(
                "Commit blocked",
                "Another client owns a durable submission attempt",
            );
            return;
        }
        let set = self
            .state
            .borrow()
            .change_sets
            .iter()
            .find(|set| set.id == change_set_id)
            .cloned()
            .expect("submission target must exist");
        let selected_ticket_ids = changes.into_iter().map(|change| change.id).collect();
        let service = self.service.clone();
        let (sender, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("finery-jira-commit".into())
            .spawn(move || {
                let _ =
                    sender.send(service.submit_change_set_from_snapshot(set, selected_ticket_ids));
            }) {
            Ok(_) => {
                self.response = Some(receiver);
                ctx.request_tick();
            }
            Err(error) => {
                self.state.borrow_mut().end_submission(&change_set_id);
                self.notify_error(
                    "Commit failed",
                    &format!("Could not start Jira commit: {error}"),
                );
            }
        }
    }

    pub(super) fn drain_results(&mut self) -> bool {
        let Some(response) = self.response.as_ref() else {
            return false;
        };
        let result = match response.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                Err("submission worker stopped before Jira returned a result".into())
            }
        };
        self.response = None;
        let active_change_set = self.state.borrow().active_change_set.clone();
        if let Some(change_set_id) = active_change_set.as_deref() {
            self.state.borrow_mut().end_submission(change_set_id);
        }
        match result {
            Ok(submitted) => {
                self.state
                    .borrow_mut()
                    .replace_change_sets(submitted.change_sets);
                self.report_outcome(&submitted.response.outcome);
            }
            Err(error) => {
                self.service.load_composer_catalog();
                self.notify_error("Commit failed", &error);
            }
        }
        true
    }

    fn report_outcome(&mut self, outcome: &SubmitChangeSetOutcome) {
        match outcome {
            SubmitChangeSetOutcome::PreflightError { message } => {
                self.preflight_error = Some(message.clone());
            }
            SubmitChangeSetOutcome::Conflict { ticket_ids } => {
                self.service.report_notification(tuicore::Notification::warning(
                    "Commit cancelled",
                    format!(
                        "Jira changed since this change set was composed: {}. Refresh those tickets before retrying.",
                        ticket_ids.join(", ")
                    ),
                ));
            }
            SubmitChangeSetOutcome::Completed { tickets } => {
                for ticket in tickets.iter().filter(|ticket| !ticket.submitted) {
                    self.notify_error(
                        &format!("{} failed", ticket.ticket_id),
                        ticket
                            .message
                            .as_deref()
                            .unwrap_or("Jira did not return a failure message"),
                    );
                }
                let submitted = tickets.iter().filter(|ticket| ticket.submitted).count();
                if submitted == 1 {
                    self.service
                        .report_notification(tuicore::Notification::success(
                            "Ticket committed",
                            "Jira submission completed",
                        ));
                } else if submitted > 1 {
                    self.service
                        .report_notification(tuicore::Notification::success(
                            "Tickets committed",
                            format!("{submitted} tickets committed"),
                        ));
                }
            }
        }
    }

    fn notify_error(&self, title: impl Into<String>, message: &str) {
        self.service
            .report_notification(tuicore::Notification::error(title, message));
    }
}
