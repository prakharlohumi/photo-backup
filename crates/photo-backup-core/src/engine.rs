use crate::auth::{load_or_authorize, GoogleAuthConfig};
use crate::model::{BackupSnapshot, FileKind, ItemStatus};
use crate::retry::RetryPolicy;
use crate::scanner::scan_source;
use crate::state::StateStore;
use crate::uploader::GooglePhotosUploader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct BackupSettings {
    pub source_root: PathBuf,
    pub state_dir: PathBuf,
    pub client_id: String,
    pub client_secret: Option<String>,
}

#[derive(Debug)]
pub struct BackupController {
    settings: BackupSettings,
    store: Arc<StateStore>,
    paused: Arc<AtomicBool>,
    stop_requested: Arc<AtomicBool>,
    runtime: Arc<Mutex<RuntimeState>>,
}

#[derive(Debug, Default)]
struct RuntimeState {
    running: bool,
    current_item: Option<String>,
    last_message: Option<String>,
    worker_join: Option<thread::JoinHandle<anyhow::Result<()>>>,
}

impl BackupController {
    pub fn new(settings: BackupSettings) -> anyhow::Result<Self> {
        let store = Arc::new(StateStore::open(&settings.state_dir)?);
        Ok(Self {
            settings,
            store,
            paused: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            runtime: Arc::new(Mutex::new(RuntimeState::default())),
        })
    }

    pub fn start(&self) -> anyhow::Result<()> {
        {
            let runtime = self.runtime.lock().unwrap();
            if runtime.running {
                self.paused.store(false, Ordering::SeqCst);
                return Ok(());
            }
        }

        self.stop_requested.store(false, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);

        let settings = self.settings.clone();
        let store = self.store.clone();
        let paused = self.paused.clone();
        let stop_requested = self.stop_requested.clone();
        let runtime = self.runtime.clone();

        let handle =
            thread::spawn(move || run_worker(settings, store, paused, stop_requested, runtime));

        let mut runtime_guard = self.runtime.lock().unwrap();
        runtime_guard.running = true;
        runtime_guard.worker_join = Some(handle);
        runtime_guard.last_message = Some(String::from("backup worker started"));
        Ok(())
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        if let Ok(mut runtime) = self.runtime.lock() {
            runtime.last_message = Some(String::from("backup paused"));
        }
    }

    pub fn resume(&self) -> anyhow::Result<()> {
        self.paused.store(false, Ordering::SeqCst);
        if let Ok(mut runtime) = self.runtime.lock() {
            runtime.last_message = Some(String::from("backup resumed"));
        }
        self.start()
    }

    pub fn stop(&self) -> anyhow::Result<()> {
        self.stop_requested.store(true, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
        let join = {
            let mut runtime = self.runtime.lock().unwrap();
            runtime.running = false;
            runtime.worker_join.take()
        };
        if let Some(handle) = join {
            let _ = handle.join();
        }
        Ok(())
    }

    pub fn snapshot(&self) -> anyhow::Result<BackupSnapshot> {
        let checkpoint = self.store.load_checkpoint()?;
        let mut snapshot = self.store.snapshot(&checkpoint);
        snapshot.paused = self.paused.load(Ordering::SeqCst);
        snapshot.running = self.runtime.lock().unwrap().running;
        snapshot.current_item = self
            .runtime
            .lock()
            .unwrap()
            .current_item
            .clone()
            .or(snapshot.current_item);
        snapshot.last_message = self.runtime.lock().unwrap().last_message.clone();
        Ok(snapshot)
    }

    pub fn refresh_state(&self) -> anyhow::Result<()> {
        let scan = scan_source(&self.settings.source_root)?;
        let checkpoint = self.store.load_checkpoint()?;
        let (manifest, checkpoint) = self.store.merge_scan(scan, checkpoint)?;
        self.store.save_manifest(&manifest)?;
        self.store.save_checkpoint(&checkpoint)?;
        self.store
            .append_event("scan", format!("scanned {} items", manifest.items.len()))?;
        Ok(())
    }

    pub fn finalize(self) -> anyhow::Result<()> {
        let _ = self.stop();
        Ok(())
    }

    pub fn state_dir(&self) -> &Path {
        &self.settings.state_dir
    }
}

fn run_worker(
    settings: BackupSettings,
    store: Arc<StateStore>,
    paused: Arc<AtomicBool>,
    stop_requested: Arc<AtomicBool>,
    runtime: Arc<Mutex<RuntimeState>>,
) -> anyhow::Result<()> {
    let auth = load_or_authorize(&GoogleAuthConfig {
        client_id: settings.client_id.clone(),
        client_secret: settings.client_secret.clone(),
        token_cache_path: settings.state_dir.join("google_token.json"),
    })?;
    let uploader = GooglePhotosUploader::new(auth);
    let retry_policy = RetryPolicy::default();

    let scan = scan_source(&settings.source_root)?;
    let checkpoint = store.load_checkpoint()?;
    let (manifest, mut checkpoint) = store.merge_scan(scan, checkpoint)?;
    store.save_manifest(&manifest)?;

    for item in checkpoint.items.values_mut() {
        if matches!(item.kind, FileKind::Unsupported) {
            item.status = ItemStatus::Skipped;
        } else if matches!(item.status, ItemStatus::Discovered | ItemStatus::Failed) {
            item.status = ItemStatus::Queued;
        }
    }
    store.save_checkpoint(&checkpoint)?;
    store.append_event("scan", format!("prepared {} items", checkpoint.items.len()))?;

    let keys: Vec<String> = checkpoint.items.keys().cloned().collect();
    for key in keys {
        if stop_requested.load(Ordering::SeqCst) {
            break;
        }
        wait_while_paused(&paused, &stop_requested);
        if stop_requested.load(Ordering::SeqCst) {
            break;
        }

        let snapshot_item = {
            let mut checkpoint = store.load_checkpoint()?;
            let snapshot_item = {
                let item = match checkpoint.items.get_mut(&key) {
                    Some(item) => item,
                    None => continue,
                };

                if matches!(item.status, ItemStatus::Committed | ItemStatus::Skipped) {
                    continue;
                }
                if matches!(item.kind, FileKind::Unsupported) {
                    item.status = ItemStatus::Skipped;
                    item.clone()
                } else {
                    item.status = ItemStatus::Uploading;
                    item.attempts = item.attempts.saturating_add(1);
                    item.last_error = None;
                    item.clone()
                }
            };
            store.save_checkpoint(&checkpoint)?;
            snapshot_item
        };

        {
            let mut runtime = runtime.lock().unwrap();
            runtime.current_item = Some(key.clone());
            runtime.last_message = Some(format!(
                "uploading {}",
                snapshot_item.relative_path.display()
            ));
        }

        let mut attempt = 0u32;
        let mut succeeded = false;
        loop {
            if stop_requested.load(Ordering::SeqCst) {
                break;
            }
            wait_while_paused(&paused, &stop_requested);
            if stop_requested.load(Ordering::SeqCst) {
                break;
            }

            match uploader.upload_item(&snapshot_item) {
                Ok(remote_media_id) => {
                    let mut checkpoint = store.load_checkpoint()?;
                    if let Some(item) = checkpoint.items.get_mut(&key) {
                        item.status = ItemStatus::Committed;
                        item.remote_media_id = Some(remote_media_id);
                        item.upload_token = None;
                        item.last_error = None;
                        item.next_retry_at_unix_secs = None;
                    }
                    store.save_checkpoint(&checkpoint)?;
                    store.append_event("committed", format!("uploaded {key}"))?;
                    succeeded = true;
                    break;
                }
                Err(error) => {
                    attempt = attempt.saturating_add(1);
                    if !retry_policy.should_retry(attempt) {
                        let mut checkpoint = store.load_checkpoint()?;
                        if let Some(item) = checkpoint.items.get_mut(&key) {
                            item.status = ItemStatus::Failed;
                            item.last_error = Some(error.to_string());
                        }
                        store.save_checkpoint(&checkpoint)?;
                        store.append_event("failed", format!("{key}: {error}"))?;
                        break;
                    }

                    let delay = retry_policy.delay_for_attempt(attempt);
                    let next_retry_at = now_plus(delay);
                    let mut checkpoint = store.load_checkpoint()?;
                    if let Some(item) = checkpoint.items.get_mut(&key) {
                        item.status = ItemStatus::Retrying;
                        item.last_error = Some(error.to_string());
                        item.next_retry_at_unix_secs = Some(next_retry_at);
                    }
                    store.save_checkpoint(&checkpoint)?;
                    store.append_event(
                        "retry",
                        format!("{key}: attempt {} waiting {:?}", attempt, delay),
                    )?;
                    sleep_with_interrupt(delay, &paused, &stop_requested);
                }
            }
        }

        let mut runtime = runtime.lock().unwrap();
        if succeeded {
            runtime.last_message = Some(format!("uploaded {key}"));
        }
        runtime.current_item = None;
    }

    let mut runtime = runtime.lock().unwrap();
    runtime.running = false;
    runtime.worker_join = None;
    runtime.last_message = Some(String::from("backup worker stopped"));
    Ok(())
}

fn wait_while_paused(paused: &Arc<AtomicBool>, stop_requested: &Arc<AtomicBool>) {
    while paused.load(Ordering::SeqCst) && !stop_requested.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(250));
    }
}

fn sleep_with_interrupt(
    duration: Duration,
    paused: &Arc<AtomicBool>,
    stop_requested: &Arc<AtomicBool>,
) {
    let mut elapsed = Duration::from_millis(0);
    while elapsed < duration && !stop_requested.load(Ordering::SeqCst) {
        wait_while_paused(paused, stop_requested);
        let step = Duration::from_millis(250);
        thread::sleep(step);
        elapsed = elapsed.saturating_add(step);
    }
}

fn now_plus(duration: Duration) -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .checked_add(duration)
        .and_then(|ts| ts.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn controller_can_read_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.jpg"), b"abc").unwrap();
        let settings = BackupSettings {
            source_root: dir.path().to_path_buf(),
            state_dir: dir.path().join("state"),
            client_id: String::from("client"),
            client_secret: None,
        };
        let controller = BackupController::new(settings).unwrap();
        controller.refresh_state().unwrap();
        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.total_items, 1);
    }
}
