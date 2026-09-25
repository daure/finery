use std::{fs, io::Read, net::TcpListener};

use base64::Engine;
use ratatui::layout::Rect;
use tuicore::{ImageProtocol, LayoutCtx, TuiNode};

use super::*;

const TEST_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAGAAAAAwCAIAAABhdOiYAAAAf0lEQVR42u3RMRGAQBADwBdBTU1NjTDkvAhcffMSMJFrbjYTA9mM47wjvd4v0j2fSFcoAxAgQIAAAQIECBAgQIAKgLoOSx0PCBAgQIAAAQIECBAgQBVAXYeljgcECBAgQIAAAQIECBCgCqCuw1LHAwIECBAgQIAAAQIECFAB0A/Lrzglvf/PRwAAAABJRU5ErkJggg==";

#[test]
fn viewer_files_preserve_original_bytes_and_use_private_unique_image_paths() {
    let data = base64::engine::general_purpose::STANDARD
        .decode(TEST_PNG)
        .unwrap();
    let first = image_viewer_file(&data).unwrap();
    let second = image_viewer_file(&data).unwrap();
    assert_ne!(first.path(), second.path());
    assert_eq!(first.path().extension().unwrap(), "png");
    assert_eq!(fs::read(first.path()).unwrap(), data);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            first.as_file().metadata().unwrap().permissions().mode() & 0o077,
            0
        );
    }
    assert!(image_viewer_file(b"not an image").is_err());
}

#[test]
fn jira_images_validate_origin_authenticate_and_register_image_interaction() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let service = AppService::for_tests();
    {
        let mut settings = service.settings.write().unwrap();
        settings.jira_base_url = url.clone();
        settings.jira_email = "user".into();
        settings.jira_api_token = "token".into();
    }
    for foreign in [
        "https://127.0.0.1/image",
        "http://localhost/image",
        "http://127.0.0.1:1/image",
    ] {
        assert_eq!(
            service.load_jira_attachment_image(foreign).unwrap_err(),
            "Jira attachment URL does not match the configured Jira site"
        );
    }
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        assert!(
            String::from_utf8(request)
                .unwrap()
                .to_ascii_lowercase()
                .contains("authorization: basic dxnlcjp0b2tlbg==")
        );
        let data = base64::engine::general_purpose::STANDARD
            .decode(TEST_PNG)
            .unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            data.len()
        )
        .unwrap();
        stream.write_all(&data).unwrap();
    });
    let mut image = service
        .load_jira_attachment_image(&format!("{url}/image"))
        .unwrap()
        .protocol(ImageProtocol::Kitty);
    server.join().unwrap();
    assert_eq!(image.dimensions(), (96, 48));
    let mut layout = LayoutCtx::new();
    <Image as TuiNode<()>>::layout(&mut image, Rect::new(0, 0, 10, 3), &mut layout);
    assert_eq!(layout.hit_regions().len(), 1);
}
