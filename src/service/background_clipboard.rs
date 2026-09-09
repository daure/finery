use std::sync::mpsc::{self, Receiver, TryRecvError};

use super::AppService;

#[cfg(test)]
#[path = "tests/background_clipboard.rs"]
mod tests;

#[derive(Default)]
pub(super) struct BackgroundClipboard {
    pending: Option<Receiver<Result<String, String>>>,
}

impl AppService {
    pub(crate) fn copy_in_background(
        &self,
        load: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) {
        let mut clipboard = self
            .background_clipboard
            .lock()
            .expect("clipboard lock poisoned");
        // A request owns its receiver, so a newer copy discards late results from older copies.
        clipboard.pending = None;
        let (sender, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("finery-report-copy".into())
            .spawn(move || {
                let _ = sender.send(load());
            }) {
            Ok(_) => clipboard.pending = Some(receiver),
            Err(error) => self.report_error(format!("Could not copy sprint report: {error}")),
        }
    }

    pub(crate) fn clipboard_pending(&self) -> bool {
        self.background_clipboard
            .lock()
            .expect("clipboard lock poisoned")
            .pending
            .is_some()
    }

    pub(crate) fn cancel_pending_clipboard(&self) {
        self.background_clipboard
            .lock()
            .expect("clipboard lock poisoned")
            .pending = None;
    }

    pub(crate) fn take_pending_clipboard(&self) -> Option<String> {
        let mut clipboard = self
            .background_clipboard
            .lock()
            .expect("clipboard lock poisoned");
        let receiver = clipboard.pending.as_ref()?;
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("Report copy worker disconnected".into()),
        };
        clipboard.pending = None;
        match result {
            Ok(text) => Some(text),
            Err(error) => {
                self.report_error(format!("Could not copy sprint report: {error}"));
                None
            }
        }
    }
}
