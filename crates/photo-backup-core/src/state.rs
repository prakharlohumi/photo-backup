use crate::model::{BackupItem, BackupItemSummary, BackupSnapshot, ItemStatus, ScanResult};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub timestamp_unix_secs: u64,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CheckpointFile {
    pub version: u32,
    pub source_root: PathBuf,
    pub items: BTreeMap<String, BackupItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ManifestFile {
    pub version: u32,
    pub source_root: PathBuf,
    pub items: BTreeMap<String, BackupItem>,
}

#[derive(Debug, Clone)]
pub struct StateStore {
    pub root_dir: PathBuf,
    manifest_path: PathBuf,
    checkpoint_path: PathBuf,
    events_path: PathBuf,
}

impl StateStore {
    pub fn open(state_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root_dir = state_dir.as_ref().to_path_buf();
        fs::create_dir_all(&root_dir)
            .with_context(|| format!("failed to create state dir {}", root_dir.display()))?;
        let manifest_path = root_dir.join("manifest.json");
        let checkpoint_path = root_dir.join("checkpoint.json");
        let events_path = root_dir.join("events.jsonl");
        Ok(Self {
            root_dir,
            manifest_path,
            checkpoint_path,
            events_path,
        })
    }

    pub fn load_manifest(&self) -> anyhow::Result<ManifestFile> {
        read_json_or_default(&self.manifest_path)
    }

    pub fn load_checkpoint(&self) -> anyhow::Result<CheckpointFile> {
        read_json_or_default(&self.checkpoint_path)
    }

    pub fn save_manifest(&self, manifest: &ManifestFile) -> anyhow::Result<()> {
        write_json_atomic(&self.manifest_path, manifest)
    }

    pub fn save_checkpoint(&self, checkpoint: &CheckpointFile) -> anyhow::Result<()> {
        write_json_atomic(&self.checkpoint_path, checkpoint)
    }

    pub fn clear_all(&self) -> anyhow::Result<()> {
        remove_if_exists(&self.manifest_path)?;
        remove_if_exists(&self.checkpoint_path)?;
        remove_if_exists(&self.events_path)?;
        Ok(())
    }

    pub fn append_event(
        &self,
        kind: impl Into<String>,
        message: impl Into<String>,
    ) -> anyhow::Result<()> {
        let record = EventRecord {
            timestamp_unix_secs: now_unix_secs(),
            kind: kind.into(),
            message: message.into(),
        };
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.events_path)
            .with_context(|| format!("failed to open {}", self.events_path.display()))?;
        serde_json::to_writer(&mut file, &record)?;
        file.write_all(b"\n")?;
        Ok(())
    }

    pub fn merge_scan(
        &self,
        scan: ScanResult,
        mut checkpoint: CheckpointFile,
    ) -> anyhow::Result<(ManifestFile, CheckpointFile)> {
        let mut manifest = ManifestFile {
            version: 1,
            source_root: scan.source_root.clone(),
            items: BTreeMap::new(),
        };

        for item in scan.items {
            let key = item.key();
            manifest.items.insert(key.clone(), item.clone());

            let entry = checkpoint.items.entry(key).or_insert(item.clone());
            if entry.fingerprint != item.fingerprint {
                *entry = item;
                entry.status = ItemStatus::Discovered;
                entry.attempts = 0;
                entry.remote_media_id = None;
                entry.upload_token = None;
                entry.last_error = None;
                entry.next_retry_at_unix_secs = None;
            } else if matches!(entry.status, ItemStatus::Failed | ItemStatus::Retrying) {
                entry.status = ItemStatus::Queued;
            }
        }

        checkpoint.version = 1;
        checkpoint.source_root = scan.source_root;
        Ok((manifest, checkpoint))
    }

    pub fn snapshot(&self, checkpoint: &CheckpointFile) -> BackupSnapshot {
        let mut snapshot = BackupSnapshot {
            source_root: checkpoint.source_root.clone(),
            total_items: checkpoint.items.len(),
            ..BackupSnapshot::default()
        };

        for (key, item) in &checkpoint.items {
            match item.status {
                ItemStatus::Discovered => snapshot.discovered += 1,
                ItemStatus::Queued => snapshot.queued += 1,
                ItemStatus::Uploading => snapshot.uploading += 1,
                ItemStatus::Uploaded => snapshot.uploaded += 1,
                ItemStatus::Committed => snapshot.committed += 1,
                ItemStatus::Skipped => snapshot.skipped += 1,
                ItemStatus::Retrying => snapshot.retrying += 1,
                ItemStatus::Failed => snapshot.failed += 1,
            }

            if matches!(item.status, ItemStatus::Skipped) {
                snapshot.skipped_items.push(summary_for(key, item));
            }
            if matches!(item.status, ItemStatus::Failed) {
                snapshot.failed_items.push(summary_for(key, item));
            }
            if snapshot.current_item.is_none()
                && matches!(item.status, ItemStatus::Uploading | ItemStatus::Retrying)
            {
                snapshot.current_item = Some(key.clone());
            }
        }

        snapshot
    }
}

fn summary_for(key: &str, item: &BackupItem) -> BackupItemSummary {
    BackupItemSummary {
        path: key.to_string(),
        status: item.status.clone(),
        attempts: item.attempts,
        error: item.last_error.clone(),
        reason: item.skip_reason.clone(),
        remote_media_id: item.remote_media_id.clone(),
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_json_or_default<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?)
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let parent = path.parent().context("missing parent directory")?;
    fs::create_dir_all(parent)?;
    let temp_path = parent.join(format!(
        ".{}.tmp",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("state")
    ));
    let data = serde_json::to_vec_pretty(value)?;
    fs::write(&temp_path, data)?;
    fs::rename(&temp_path, path)?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileFingerprint, FileKind, ItemStatus};
    use std::path::PathBuf;

    #[test]
    fn merges_and_resets_changed_items() {
        let store = StateStore::open(tempfile::tempdir().unwrap().path()).unwrap();
        let scan = ScanResult {
            source_root: PathBuf::from("/tmp/root"),
            items: vec![BackupItem {
                relative_path: PathBuf::from("a.jpg"),
                absolute_path: PathBuf::from("/tmp/root/a.jpg"),
                file_name: String::from("a.jpg"),
                kind: FileKind::Photo,
                fingerprint: FileFingerprint {
                    size: 1,
                    modified_unix_secs: Some(1),
                },
                status: ItemStatus::Discovered,
                attempts: 0,
                remote_media_id: None,
                upload_token: None,
                last_error: None,
                skip_reason: None,
                next_retry_at_unix_secs: None,
            }],
        };
        let checkpoint = CheckpointFile::default();
        let (_, checkpoint) = store.merge_scan(scan, checkpoint).unwrap();
        assert_eq!(checkpoint.items.len(), 1);
    }
}
