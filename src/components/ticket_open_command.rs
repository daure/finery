use tuicore::{EventCtx, TuiEvent};

use crate::service::AppService;

pub(crate) fn handle(
    service: &AppService,
    event: &TuiEvent,
    ticket_key: Option<&str>,
    ctx: &mut EventCtx<()>,
) -> Option<bool> {
    let TuiEvent::Key(key) = event else {
        return None;
    };
    if !service
        .settings()
        .read()
        .is_ok_and(|settings| settings.open_command_key.matches(*key))
    {
        return None;
    }
    let triggered = ticket_key.is_some_and(|key| service.run_open_command(key));
    ctx.stop_propagation();
    Some(triggered)
}
