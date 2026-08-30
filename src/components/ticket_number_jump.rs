use std::time::Duration;

use tuicore::{Key, KeyEvent};

const TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(crate) struct TicketNumberJump {
    prefix: String,
    elapsed: Duration,
}

impl TicketNumberJump {
    pub(crate) fn push(&mut self, key: KeyEvent) -> bool {
        let Key::Char(digit) = key.code else {
            return false;
        };
        if !key.modifiers.is_empty() || !digit.is_ascii_digit() {
            return false;
        }
        self.prefix.push(digit);
        self.elapsed = Duration::ZERO;
        true
    }

    pub(crate) fn query(&self) -> Option<&str> {
        (!self.prefix.is_empty()).then_some(&self.prefix)
    }

    pub(crate) fn clear(&mut self) {
        self.prefix.clear();
        self.elapsed = Duration::ZERO;
    }

    pub(crate) fn accepts(&self, key: KeyEvent) -> bool {
        self.query().is_some() && matches!(key.code, Key::Enter) && key.modifiers.is_empty()
    }

    pub(crate) fn cancels(&self, key: KeyEvent) -> bool {
        self.query().is_some()
            && (matches!(key.code, Key::Esc)
                || matches!(key.code, Key::Char('[')
                    if key.modifiers == tuicore::KeyModifiers::CONTROL))
    }

    pub(crate) fn advance(&mut self, dt: Duration) -> bool {
        if self.prefix.is_empty() {
            return false;
        }
        self.elapsed = self.elapsed.saturating_add(dt);
        if self.elapsed < TIMEOUT {
            return false;
        }
        self.clear();
        true
    }

    pub(crate) fn remaining(&self) -> Option<Duration> {
        (!self.prefix.is_empty()).then(|| TIMEOUT.saturating_sub(self.elapsed))
    }
}

pub(crate) fn ticket_number_matches(key: &str, number: &str) -> bool {
    key.rsplit_once('-')
        .is_some_and(|(_, suffix)| suffix.starts_with(number))
}

pub(crate) fn exact_ticket_number_matches(key: &str, number: &str) -> bool {
    key.rsplit_once('-')
        .is_some_and(|(_, suffix)| suffix == number)
}
