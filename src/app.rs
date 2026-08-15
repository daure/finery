use tuicore::{Flex, FlexItem, Tab, Tabs, TabsVariant};

use crate::{components, pages};

pub(crate) fn root() -> Flex<()> {
    let pages = Tabs::new(vec![
        Tab::new("Backlog", pages::backlog::page()),
        Tab::new("Sprint", pages::sprint::page()),
        Tab::new("Issues", pages::issues::page()),
        Tab::new("Composer", pages::composer::page()),
    ])
    .variant(TabsVariant::OneRow);

    Flex::column()
        .child("pages", pages, FlexItem::fill(1))
        .child(
            "status",
            components::status_bar::status_bar(),
            FlexItem::fixed(1),
        )
}
