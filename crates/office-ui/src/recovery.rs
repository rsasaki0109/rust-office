//! Typed recovery copies, isolated by live-session file locks.
use office_core::Document;
use office_format::{DocumentFormat, JsonFormat};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

pub const INTERVAL: Duration = Duration::from_secs(15);
const MAX_SNAPSHOT: u64 = 32 * 1024 * 1024;
const SNAPSHOT: &str = "snapshot.roffice.json";
const SOURCE: &str = "source.txt";
const LOCK: &str = "session.lock";

pub fn default_directory() -> Result<PathBuf, String> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
    }
    .ok_or_else(|| "No user state directory is configured".to_string())?;
    if !base.is_absolute() {
        return Err("The user state directory must be absolute".into());
    }
    Ok(base.join("rust-office/recovery/writer"))
}

pub struct Entry {
    directory: PathBuf,
    _lease: File,
    pub label: String,
    modified: SystemTime,
}
pub trait RecoveryData: serde::Serialize {
    const PREFIX: &'static str;
    fn validate(&self) -> Result<(), String>;
    fn decode(bytes: &[u8]) -> Result<Self, String>
    where
        Self: Sized;
}
impl RecoveryData for Document {
    const PREFIX: &'static str = "writer-";
    fn validate(&self) -> Result<(), String> {
        if self.sections.is_empty()
            || self.format_version > Document::CURRENT_FORMAT_VERSION
            || self.sections.iter().any(|s| !s.page_style.is_valid())
        {
            Err("Invalid recovery document version or sections".into())
        } else {
            Ok(())
        }
    }
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        JsonFormat
            .load_from_reader(&mut io::Cursor::new(bytes))
            .map_err(|e| e.to_string())
    }
}

pub struct Recovery<T: RecoveryData = Document> {
    kind: std::marker::PhantomData<T>,
    root: PathBuf,
    own: PathBuf,
    lease: Option<File>,
    pub entries: Vec<Entry>,
    pub has_snapshot: bool,
    last_attempt: Option<Instant>,
    pub cleanup_warning: Option<String>,
}
fn regular(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(m) => Ok(m.is_file()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}
fn protect_target(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(m) if !m.is_file() => Err("Recovery targets must be regular files".into()),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
impl<T: RecoveryData> Recovery<T> {
    pub fn new(root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let own = tempfile::Builder::new()
            .prefix(T::PREFIX)
            .tempdir_in(root)
            .map_err(|e| e.to_string())?
            .keep();
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(own.join(LOCK))
            .map_err(|e| e.to_string())?;
        lease.try_lock().map_err(|e| e.to_string())?;
        let mut this = Self {
            kind: std::marker::PhantomData,
            root: root.into(),
            own,
            lease: Some(lease),
            entries: Vec::new(),
            has_snapshot: false,
            last_attempt: None,
            cleanup_warning: None,
        };
        this.refresh()?;
        Ok(this)
    }
    pub fn refresh(&mut self) -> Result<(), String> {
        self.entries.clear();
        for candidate in fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let candidate = candidate.map_err(|e| e.to_string())?;
            if !candidate.file_type().map_err(|e| e.to_string())?.is_dir()
                || !candidate
                    .file_name()
                    .to_string_lossy()
                    .starts_with(T::PREFIX)
            {
                continue;
            }
            let directory = candidate.path();
            if directory == self.own || !regular(&directory.join(SNAPSHOT))? {
                continue;
            }
            // Never claim a live process's copy or follow a symlink lock file.
            if !regular(&directory.join(LOCK))? {
                continue;
            }
            let lease = match OpenOptions::new()
                .read(true)
                .write(true)
                .open(directory.join(LOCK))
            {
                Ok(f) => f,
                Err(_) => continue,
            };
            if lease.try_lock().is_err() {
                continue;
            }
            let source = directory.join(SOURCE);
            let label = if regular(&source)? {
                File::open(source)
                    .ok()
                    .and_then(|file| {
                        let mut value = String::new();
                        file.take(4096)
                            .read_to_string(&mut value)
                            .ok()
                            .map(|_| value)
                    })
                    .unwrap_or_else(|| "Unsaved document".into())
            } else {
                "Unsaved document".into()
            };
            let modified = fs::metadata(directory.join(SNAPSHOT))
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            self.entries.push(Entry {
                directory,
                _lease: lease,
                label,
                modified,
            });
            if self.entries.len() >= 100 {
                break;
            }
        }
        self.entries.sort_by_key(|e| std::cmp::Reverse(e.modified));
        Ok(())
    }
    pub fn postpone(&mut self) {
        self.entries.clear();
    }
    pub fn due(&self, now: Instant) -> bool {
        self.last_attempt
            .is_none_or(|last| now.saturating_duration_since(last) >= INTERVAL)
    }
    pub fn tick(&mut self, doc: &T, dirty: bool, label: &str, now: Instant) -> Result<(), String> {
        if !dirty {
            return self.clear();
        }
        if !self.due(now) {
            return Ok(());
        }
        self.last_attempt = Some(now);
        self.save(doc, label)
    }
    fn save(&mut self, doc: &T, label: &str) -> Result<(), String> {
        self.save_with_limit(doc, label, MAX_SNAPSHOT)
    }
    fn save_with_limit(&mut self, doc: &T, label: &str, limit: u64) -> Result<(), String> {
        doc.validate()?;
        let snapshot = self.own.join(SNAPSHOT);
        let source = self.own.join(SOURCE);
        protect_target(&snapshot)?;
        protect_target(&source)?;
        office_core::storage::atomic_save::<io::Error>(&snapshot, |file| {
            let mut bounded = BoundedWriter {
                writer: file,
                remaining: limit,
            };
            serde_json::to_writer(&mut bounded, doc).map_err(io::Error::other)
        })
        .map_err(|e| e.to_string())?;
        self.has_snapshot = true;
        office_core::storage::atomic_write(&source, label.as_bytes()).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn restore(&mut self, index: usize) -> Result<T, String> {
        let entry = self
            .entries
            .get(index)
            .ok_or_else(|| "Recovery copy is no longer available".to_string())?;
        let path = entry.directory.join(SNAPSHOT);
        if !regular(&path)? {
            return Err("Recovery copy must be a regular file".into());
        }
        let file = File::open(&path).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() > MAX_SNAPSHOT {
            return Err("Recovery copy exceeds the 32 MiB limit".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_SNAPSHOT + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_SNAPSHOT {
            return Err("Recovery copy exceeds the 32 MiB limit".into());
        }
        let doc = T::decode(&bytes)?;
        doc.validate()?;
        let label = entry.label.clone();
        // Transfer durably before retiring the old copy. A second crash remains recoverable.
        self.save(&doc, &label)?;
        self.cleanup_warning = self.delete(index).err();
        self.last_attempt = Some(Instant::now());
        Ok(doc)
    }
    pub fn delete(&mut self, index: usize) -> Result<(), String> {
        let entry = self
            .entries
            .get(index)
            .ok_or_else(|| "Recovery copy is no longer available".to_string())?;
        fs::remove_file(entry.directory.join(SNAPSHOT)).map_err(|e| e.to_string())?;
        let entry = self.entries.remove(index);
        let directory = entry.directory.clone();
        drop(entry);
        // Delete only known files in this retired session; unknown files keep the directory.
        let _ = fs::remove_file(directory.join(SOURCE));
        let _ = fs::remove_file(directory.join(LOCK));
        let _ = fs::remove_dir(directory);
        Ok(())
    }
    pub fn clear(&mut self) -> Result<(), String> {
        if self.has_snapshot {
            match fs::remove_file(self.own.join(SNAPSHOT)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
            let _ = fs::remove_file(self.own.join(SOURCE));
            self.has_snapshot = false;
        }
        self.last_attempt = None;
        Ok(())
    }
}
impl<T: RecoveryData> Drop for Recovery<T> {
    fn drop(&mut self) {
        self.lease.take();
        if !self.has_snapshot {
            let _ = fs::remove_file(self.own.join(SOURCE));
            let _ = fs::remove_file(self.own.join(LOCK));
            let _ = fs::remove_dir(&self.own);
        }
    }
}
struct BoundedWriter<W: Write> {
    writer: W,
    remaining: u64,
}
impl<W: Write> Write for BoundedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(io::Error::other("Recovery copy exceeds the 32 MiB limit"));
        }
        let written = self.writer.write(bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_sessions_are_skipped_and_crashed_copies_are_claimed_once() {
        let dir = tempfile::tempdir().unwrap();
        let doc = Document::with_text("日本語 recovery");
        let mut first = Recovery::<Document>::new(dir.path()).unwrap();
        first.save(&doc, "original.docx").unwrap();
        let mut second = Recovery::<Document>::new(dir.path()).unwrap();
        assert!(second.entries.is_empty());
        drop(first);
        second.refresh().unwrap();
        assert_eq!(second.entries.len(), 1);
        let third = Recovery::<Document>::new(dir.path()).unwrap();
        assert!(third.entries.is_empty());
        drop(third);
        second.postpone();
        second.refresh().unwrap();
        assert_eq!(second.entries.len(), 1);
    }
    #[test]
    fn restored_data_is_durable_before_old_copy_is_retired() {
        let dir = tempfile::tempdir().unwrap();
        let doc = Document::with_text("日本語 & <text>\nsecond");
        let mut first = Recovery::<Document>::new(dir.path()).unwrap();
        first.save(&doc, "untitled").unwrap();
        drop(first);
        let mut second = Recovery::<Document>::new(dir.path()).unwrap();
        assert_eq!(second.restore(0).unwrap(), doc);
        assert!(second.entries.is_empty());
        drop(second);
        let mut third = Recovery::<Document>::new(dir.path()).unwrap();
        assert_eq!(third.entries.len(), 1);
        assert_eq!(third.restore(0).unwrap(), doc);
        third.clear().unwrap();
        drop(third);
        assert!(Recovery::<Document>::new(dir.path())
            .unwrap()
            .entries
            .is_empty());
    }
    #[test]
    fn failed_write_preserves_prior_copy_and_timer_does_not_change_document() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Recovery::<Document>::new(dir.path()).unwrap();
        let doc = Document::with_text("first");
        let now = Instant::now();
        store.tick(&doc, true, "original.odt", now).unwrap();
        let path = store.own.join(SNAPSHOT);
        let before = fs::read(&path).unwrap();
        let edited = Document::with_text("second 日本語");
        store
            .tick(&edited, true, "original.odt", now + Duration::from_secs(1))
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(store.save_with_limit(&edited, "original.odt", 4).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        store
            .tick(&edited, true, "original.odt", now + INTERVAL)
            .unwrap();
        assert_ne!(fs::read(&path).unwrap(), before);
        store.clear().unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn corrupt_or_oversized_copies_are_retained_when_restore_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = Recovery::<Document>::new(dir.path()).unwrap();
        first.save(&Document::new(), "bad").unwrap();
        let path = first.own.join(SNAPSHOT);
        drop(first);
        fs::write(&path, b"not json").unwrap();
        let mut next = Recovery::<Document>::new(dir.path()).unwrap();
        assert!(next.restore(0).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not json");
        assert_eq!(next.entries.len(), 1);
        File::create(&path)
            .unwrap()
            .set_len(MAX_SNAPSHOT + 1)
            .unwrap();
        assert!(next.restore(0).unwrap_err().contains("32 MiB"));
        assert!(path.exists());
        next.delete(0).unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn native_structure_and_embedded_bytes_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let mut doc = Document::with_text("日本語");
        doc.title = "document title".into();
        doc.ensure_header_mut()
            .runs
            .push(office_core::Run::plain("header"));
        doc.main_section_mut()
            .blocks
            .push(office_core::Block::Image(
                office_core::Image::from_embedded("image/png", vec![1, 2, 3], "image", 32.0, 24.0),
            ));
        let mut first = Recovery::<Document>::new(dir.path()).unwrap();
        first.save(&doc, "document.roffice.json").unwrap();
        drop(first);
        let mut second = Recovery::<Document>::new(dir.path()).unwrap();
        assert_eq!(second.restore(0).unwrap(), doc);
    }
}
