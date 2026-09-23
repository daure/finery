use tuicore::{EventCtx, Key, KeyEvent, KeyModifiers, TuiEvent, TuiNode};

use super::OpenCommandMenu;
use crate::service::{AppService, OpenCommandProbe};

#[test]
fn selecting_a_value_runs_the_open_command_with_that_value() {
    tuicore::init();
    let service = AppService::for_tests();
    let probe = OpenCommandProbe::new(&service);
    let mut menu = OpenCommandMenu::new(service.clone());
    service.settings().write().unwrap().open_command_enum = vec!["editor".into()];
    assert!(service.open_command("FIN-42", "Ticket title"));

    menu.open(
        service.take_open_command_request().unwrap(),
        &mut EventCtx::default(),
    );
    menu.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::NONE,
        }),
        &mut EventCtx::default(),
    );

    probe.assert_opened_with_value("FIN-42", "Ticket title", "editor");
    assert!(menu.take_close_requested());
}
