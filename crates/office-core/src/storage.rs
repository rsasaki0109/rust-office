//! Shared file replacement for document saves and exports.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

/// Write a complete document to a temporary file in the destination directory,
/// then replace the destination atomically. Failed writes leave it untouched.
/// Existing permissions and symlink destinations are preserved.
pub fn atomic_save<E: From<io::Error>>(
    path: &Path,
    write: impl FnOnce(&mut File) -> Result<(), E>,
) -> Result<(), E> {
    let target = match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::canonicalize(path)?,
        Ok(_) => path.to_path_buf(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
        Err(error) => return Err(error.into()),
    };
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".rust-office-")
        .tempfile_in(parent)?;
    match fs::metadata(&target) {
        Ok(meta) => {
            if meta.permissions().readonly() {
                return Err(
                    io::Error::new(io::ErrorKind::PermissionDenied, "file is read-only").into(),
                );
            }
            temporary.as_file().set_permissions(meta.permissions())?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    write(temporary.as_file_mut())?;
    temporary.as_file_mut().flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(&target).map_err(|error| error.error)?;
    Ok(())
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_save(path, |file| file.write_all(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_write_failure_preserves_original_and_cleans_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document.json");
        fs::write(&path, b"original").unwrap();
        let result: io::Result<()> = atomic_save(&path, |file| {
            file.write_all(b"partial")?;
            Err(io::Error::other("serialization failed"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn replaces_existing_file_and_creates_missing_parent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/document.json");
        atomic_write(&path, b"first").unwrap();
        atomic_write(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn failed_replacement_does_not_remove_destination_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("directory");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), b"original").unwrap();
        assert!(atomic_write(&path, b"new").is_err());
        assert_eq!(fs::read(path.join("keep")).unwrap(), b"original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn preserves_symlink_and_file_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document.json");
        let link = dir.path().join("link.json");
        fs::write(&path, b"original").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&path, &link).unwrap();
        atomic_write(&link, b"new").unwrap();
        assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}
