use crate::auth::TokenCache;
use crate::model::{BackupItem, FileKind};
use anyhow::Context;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::fs;

#[derive(Debug, Clone)]
pub struct GooglePhotosUploader {
    client: Client,
    token: TokenCache,
}

impl GooglePhotosUploader {
    pub fn new(token: TokenCache) -> Self {
        Self {
            client: Client::new(),
            token,
        }
    }

    pub fn refresh_token(&mut self, token: TokenCache) {
        self.token = token;
    }

    pub fn upload_item(&self, item: &BackupItem) -> anyhow::Result<String> {
        match item.kind {
            FileKind::Photo | FileKind::Video => {}
            FileKind::Unsupported => anyhow::bail!("unsupported file type"),
        }

        let body = fs::read(&item.absolute_path)
            .with_context(|| format!("failed to read {}", item.absolute_path.display()))?;
        let upload_token = self.upload_bytes(&item.file_name, &body)?;
        let media_id = self.commit_upload(&item.file_name, &upload_token)?;
        Ok(media_id)
    }

    fn upload_bytes(&self, file_name: &str, bytes: &[u8]) -> anyhow::Result<String> {
        let response = self
            .client
            .post("https://photoslibrary.googleapis.com/v1/uploads")
            .header(
                "Authorization",
                format!("Bearer {}", self.token.access_token),
            )
            .header("Content-Type", "application/octet-stream")
            .header("X-Goog-Upload-Protocol", "raw")
            .header("X-Goog-Upload-File-Name", file_name)
            .body(bytes.to_vec())
            .send()?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().unwrap_or_default();
            anyhow::bail!("upload token request failed: {status} {text}");
        }

        Ok(response.text()?.trim().to_string())
    }

    fn commit_upload(&self, file_name: &str, upload_token: &str) -> anyhow::Result<String> {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct BatchCreateRequest<'a> {
            new_media_items: Vec<NewMediaItem<'a>>,
        }

        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct NewMediaItem<'a> {
            description: &'a str,
            simple_media_item: SimpleMediaItem<'a>,
        }

        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SimpleMediaItem<'a> {
            upload_token: &'a str,
            file_name: &'a str,
        }

        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct BatchCreateResponse {
            new_media_item_results: Vec<MediaItemResult>,
        }

        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct MediaItemResult {
            media_item: Option<MediaItem>,
            status: Option<ApiStatus>,
        }

        #[derive(Debug, Deserialize)]
        struct MediaItem {
            id: String,
        }

        #[derive(Debug, Deserialize)]
        struct ApiStatus {
            message: Option<String>,
        }

        let request = BatchCreateRequest {
            new_media_items: vec![NewMediaItem {
                description: file_name,
                simple_media_item: SimpleMediaItem {
                    upload_token,
                    file_name,
                },
            }],
        };

        let response = self
            .client
            .post("https://photoslibrary.googleapis.com/v1/mediaItems:batchCreate")
            .header(
                "Authorization",
                format!("Bearer {}", self.token.access_token),
            )
            .json(&request)
            .send()?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().unwrap_or_default();
            anyhow::bail!("batch create failed: {status} {text}");
        }

        let response: BatchCreateResponse = response.json()?;
        let result = response
            .new_media_item_results
            .into_iter()
            .next()
            .context("missing media item result")?;
        if let Some(media_item) = result.media_item {
            return Ok(media_item.id);
        }
        let message = result
            .status
            .and_then(|status| status.message)
            .unwrap_or_else(|| String::from("unknown upload failure"));
        anyhow::bail!(message);
    }
}
