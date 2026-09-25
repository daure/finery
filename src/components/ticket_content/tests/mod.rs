use super::*;

const TEST_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAGAAAAAwCAIAAABhdOiYAAAAf0lEQVR42u3RMRGAQBADwBdBTU1NjTDkvAhcffMSMJFrbjYTA9mM47wjvd4v0j2fSFcoAxAgQIAAAQIECBAgQIAKgLoOSx0PCBAgQIAAAQIECBAgQBVAXYeljgcECBAgQIAAAQIECBCgCqCuw1LHAwIECBAgQIAAAQIECFAB0A/Lrzglvf/PRwAAAABJRU5ErkJggg==";

#[test]
fn loaded_inline_images_use_and_preload_the_direct_kitty_protocol() {
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Ok(Image::from_base64(TEST_PNG).unwrap()))
        .unwrap();
    let mut image = InlineImage {
        metadata: TicketImage {
            url: "https://jira.example/attachment/42".into(),
            alt: "cart.png".into(),
            width: 96,
            height: 48,
        },
        state: InlineImageState::Loading(receiver),
    };

    image.tick(Duration::ZERO, AnimationSettings::default());

    let InlineImageState::Ready(mut image) = image.state else {
        panic!("inline image should finish loading");
    };
    assert_eq!(image.graphics_protocol(), tuicore::ImageProtocol::Kitty);
    assert_ne!(Image::tick(image.as_mut()), TickResult::IDLE);
}

#[test]
fn inline_image_double_click_routes_through_scrolled_ticket_content() {
    let (opened, receiver) = mpsc::channel();
    let image = Image::from_base64(TEST_PNG)
        .unwrap()
        .protocol(ImageProtocol::Kitty)
        .on_double_click(move || opened.send(()).unwrap());
    let mut content = TicketContent {
        content: ScrollContainer::vertical(TicketDocument {
            content: Flex::column()
                .child(
                    "image",
                    InlineImage {
                        metadata: TicketImage {
                            url: String::new(),
                            alt: "cart.png".into(),
                            width: 96,
                            height: 48,
                        },
                        state: InlineImageState::Ready(Box::new(image)),
                    },
                    FlexItem::fixed(3),
                )
                .child(
                    "tail",
                    MarkdownBlock::new("Tail\n".repeat(10)),
                    FlexItem::fit_content(),
                ),
        }),
        focused: false,
    };
    let area = Rect::new(3, 4, 10, 3);
    content.layout(area, &mut LayoutCtx::new());
    content.content.scroll_to(
        tuicore::ScrollOffset::new(0, 1),
        AnimationSettings {
            enabled: false,
            ..AnimationSettings::default()
        },
    );
    let mut layout = LayoutCtx::new();
    content.layout(area, &mut layout);
    let image_path = tuicore::TreePath::new()
        .child(tuicore::ChildKey::body())
        .child("image".into());
    let hit = layout
        .hit_regions()
        .iter()
        .rev()
        .find(|hit| hit.path == image_path)
        .unwrap();
    let event = TuiEvent::Mouse(tuicore::MouseEvent {
        kind: tuicore::MouseEventKind::Down(tuicore::MouseButton::Left),
        column: hit.area.x,
        row: hit.area.y,
        modifiers: tuicore::KeyModifiers::NONE,
    });
    let route = EventRoute::new(hit.path.clone());
    content.dispatch_event(&route, &event, &mut EventCtx::default());
    assert!(receiver.try_recv().is_err());
    content.dispatch_event(&route, &event, &mut EventCtx::default());
    receiver
        .try_recv()
        .expect("double click opens the visible image");
    assert!(receiver.try_recv().is_err());
}
