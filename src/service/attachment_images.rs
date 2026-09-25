use std::{io::Write, sync::Arc, thread, time::Duration};

use tuicore::{Image, Notification};

use super::{AppService, image_extension, jira_attachment_download_error, open_external_file};

impl AppService {
    pub(crate) fn load_jira_attachment_image(&self, content_url: &str) -> Result<Image, String> {
        self.attachment_image_from_bytes(self.download_jira_image(content_url)?)
    }

    fn download_jira_image(&self, content_url: &str) -> Result<Vec<u8>, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        let (base_url, email, token) = settings.configured_jira().ok_or_else(|| {
            "Jira is not configured; add URL, email, and API token in Settings".to_string()
        })?;
        let base_url = reqwest::Url::parse(base_url).map_err(|error| error.to_string())?;
        let content_url = reqwest::Url::parse(content_url).map_err(|error| error.to_string())?;
        if content_url.scheme() != base_url.scheme()
            || content_url.host_str() != base_url.host_str()
            || content_url.port_or_known_default() != base_url.port_or_known_default()
        {
            return Err("Jira attachment URL does not match the configured Jira site".into());
        }
        let data = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| error.to_string())?
            .get(content_url)
            .basic_auth(email, Some(token))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .and_then(reqwest::blocking::Response::bytes)
            .map_err(jira_attachment_download_error)?;
        Ok(data.to_vec())
    }

    pub(crate) fn open_attachment_image(
        &self,
        attachment: &crate::store::composer::TicketAttachment,
    ) -> Result<(), String> {
        let data = match &attachment.local_data {
            Some(data) => data.clone(),
            None => self.download_jira_image(
                attachment
                    .content_url
                    .as_deref()
                    .ok_or_else(|| "Image content URL is unavailable".to_owned())?,
            )?,
        };
        Image::from_bytes(&data).map_err(|error| error.to_string())?;
        open_image_bytes(&data)
    }

    pub(crate) fn attachment_image_from_bytes(&self, data: Vec<u8>) -> Result<Image, String> {
        let image = Image::from_bytes(&data).map_err(|error| error.to_string())?;
        let data: Arc<[u8]> = data.into();
        let service = self.clone();
        Ok(image.on_double_click(move || {
            let data = Arc::clone(&data);
            let service = service.clone();
            thread::spawn(move || {
                if let Err(error) = open_image_bytes(&data) {
                    service.report_notification(Notification::error("Could not open image", error));
                }
            });
        }))
    }
}

fn open_image_bytes(data: &[u8]) -> Result<(), String> {
    let file = image_viewer_file(data)?;
    open_external_file(file.path())?;
    // Desktop launchers return before the viewer reads the file.
    file.keep().map_err(|error| error.to_string())?;
    Ok(())
}

fn image_viewer_file(data: &[u8]) -> Result<tempfile::NamedTempFile, String> {
    let extension = image_extension(data)
        .ok_or_else(|| "Image format is unavailable for the external viewer".to_string())?;
    let mut file = tempfile::Builder::new()
        .prefix("finery-image-")
        .suffix(&format!(".{extension}"))
        .tempfile()
        .map_err(|error| error.to_string())?;
    file.write_all(data).map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())?;
    Ok(file)
}

#[cfg(test)]
#[path = "tests/attachment_images.rs"]
mod tests;
