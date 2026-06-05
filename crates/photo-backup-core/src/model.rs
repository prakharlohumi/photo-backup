use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileKind {
    Photo,
    Video,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ItemStatus {
    Discovered,
    Queued,
    Uploading,
    Uploaded,
    Committed,
    Skipped,
    Retrying,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileFingerprint {
    pub size: u64,
    pub modified_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupItem {
    pub relative_path: PathBuf,
    pub absolute_path: PathBuf,
    pub file_name: String,
    pub kind: FileKind,
    pub fingerprint: FileFingerprint,
    pub status: ItemStatus,
    pub attempts: u32,
    pub remote_media_id: Option<String>,
    pub upload_token: Option<String>,
    pub last_error: Option<String>,
    pub next_retry_at_unix_secs: Option<u64>,
}

impl BackupItem {
    pub fn key(&self) -> String {
        self.relative_path.to_string_lossy().replace('\\', "/")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackupSnapshot {
    pub source_root: PathBuf,
    pub total_items: usize,
    pub discovered: usize,
    pub queued: usize,
    pub uploading: usize,
    pub uploaded: usize,
    pub committed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub retrying: usize,
    pub paused: bool,
    pub running: bool,
    pub current_item: Option<String>,
    pub last_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub source_root: PathBuf,
    pub items: Vec<BackupItem>,
}
