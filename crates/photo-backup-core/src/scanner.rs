use crate::model::{BackupItem, FileFingerprint, FileKind, ItemStatus, ScanResult};
use anyhow::Context;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

const PHOTO_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "heif", "tif", "tiff", "dng", "raw",
];
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "m4v", "avi", "mkv", "3gp", "3g2", "mpg", "mpeg", "mts", "m2ts", "webm",
];

pub fn scan_source(source_root: impl AsRef<Path>) -> anyhow::Result<ScanResult> {
    let source_root = source_root.as_ref().canonicalize().with_context(|| {
        format!(
            "failed to resolve source root {}",
            source_root.as_ref().display()
        )
    })?;

    let mut items = Vec::new();

    for entry in WalkDir::new(&source_root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }

        let metadata = fs::metadata(path)
            .with_context(|| format!("failed to read metadata for {}", path.display()))?;
        let modified_unix_secs = metadata
            .modified()
            .ok()
            .and_then(|ts| ts.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        let relative_path = path
            .strip_prefix(&source_root)
            .unwrap_or(path)
            .to_path_buf();
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| String::from("unknown"));
        let kind = classify(path);

        items.push(BackupItem {
            relative_path,
            absolute_path: path.to_path_buf(),
            file_name,
            kind,
            fingerprint: FileFingerprint {
                size: metadata.len(),
                modified_unix_secs,
            },
            status: ItemStatus::Discovered,
            attempts: 0,
            remote_media_id: None,
            upload_token: None,
            last_error: None,
            skip_reason: None,
            next_retry_at_unix_secs: None,
        });
    }

    Ok(ScanResult { source_root, items })
}

pub fn classify(path: &Path) -> FileKind {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());

    match ext.as_deref() {
        Some(ext) if PHOTO_EXTENSIONS.contains(&ext) => FileKind::Photo,
        Some(ext) if VIDEO_EXTENSIONS.contains(&ext) => FileKind::Video,
        _ => FileKind::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn classifies_common_extensions() {
        assert_eq!(classify(Path::new("a.jpg")), FileKind::Photo);
        assert_eq!(classify(Path::new("a.MP4")), FileKind::Video);
        assert_eq!(classify(Path::new("a.txt")), FileKind::Unsupported);
    }

    #[test]
    fn scans_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("sub").join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join("a.jpg"), b"abc").unwrap();
        fs::write(nested.join("b.mp4"), b"def").unwrap();

        let result = scan_source(dir.path()).unwrap();
        assert_eq!(result.items.len(), 2);
        assert!(result.items.iter().any(|item| item.file_name == "a.jpg"));
        assert!(result.items.iter().any(|item| item.file_name == "b.mp4"));
    }
}
