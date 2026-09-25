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
