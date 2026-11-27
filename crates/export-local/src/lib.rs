//! Export destinations backed by a folder: a local directory, or an SMB/NFS share that the
//! operating system has mounted.
//!
//! Fathom does not speak SMB or NFS itself. Mount the share with your OS (`mount -t cifs`,
//! `mount -t nfs`, Finder's Connect to Server, a mapped drive on Windows) and point `path` at the
//! mount point; the `smb` and `nfs` kinds then check that the share is really mounted before
//! writing, so a missing mount is an error and not a stray local folder.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use application::ports::{DestinationConfig, DestinationFactory, ExportDestination, ExportError};
use async_trait::async_trait;

/// A folder on disk.
pub struct FolderDestination {
    kind: &'static str,
    root: PathBuf,
    /// The share must be mounted: the folder has to exist already (it is never created).
    must_exist: bool,
}

impl FolderDestination {
    pub fn new(kind: &'static str, root: PathBuf, must_exist: bool) -> Self {
        Self {
            kind,
            root,
            must_exist,
        }
    }
}

/// Turns a `/`-separated relative path into path components under the root, refusing anything
/// that could leave it.
fn relative_parts(path: &str) -> Result<Vec<&str>, ExportError> {
    let bad = || {
        ExportError::Invalid(format!(
            "`{path}` is not a valid path inside the destination."
        ))
    };
    if path.is_empty() || path.starts_with('/') || path.contains('\0') {
        return Err(bad());
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(bad()),
            p if Path::new(p)
                .components()
                .any(|c| !matches!(c, Component::Normal(_))) =>
            {
                return Err(bad());
            }
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        return Err(bad());
    }
    Ok(parts)
}

fn io_error(e: &std::io::Error, what: &str) -> ExportError {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => ExportError::Denied(format!("{what}: {e}")),
        std::io::ErrorKind::NotFound => ExportError::Unreachable(format!("{what}: {e}")),
        _ => ExportError::Other(format!("{what}: {e}")),
    }
}

impl FolderDestination {
    fn check_root(&self) -> Result<(), ExportError> {
        if self.root.is_dir() {
            return Ok(());
        }
        if self.must_exist {
            return Err(ExportError::Unreachable(format!(
                "`{}` does not exist. Is the share mounted?",
                self.root.display()
            )));
        }
        Ok(())
    }

    fn put_blocking(&self, path: &str, data: &[u8]) -> Result<String, ExportError> {
        use std::io::Write;
        let parts = relative_parts(path)?;
        self.check_root()?;
        let mut target = self.root.clone();
        target.extend(&parts);
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io_error(&e, "creating the folder"))?;
            // A symlink inside the root must not lead the file somewhere else.
            let root = self
                .root
                .canonicalize()
                .map_err(|e| io_error(&e, "resolving the destination folder"))?;
            let parent = dir
                .canonicalize()
                .map_err(|e| io_error(&e, "resolving the folder"))?;
            if !parent.starts_with(&root) {
                return Err(ExportError::Invalid(format!(
                    "`{path}` leads outside the destination."
                )));
            }
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::AlreadyExists => {
                    ExportError::AlreadyExists(target.display().to_string())
                }
                _ => io_error(&e, "creating the file"),
            })?;
        if let Err(e) = file.write_all(data).and_then(|()| file.flush()) {
            drop(file);
            let _ = std::fs::remove_file(&target);
            return Err(io_error(&e, "writing the file"));
        }
        Ok(target.display().to_string())
    }

    fn test_blocking(&self) -> Result<(), ExportError> {
        self.check_root()?;
        std::fs::create_dir_all(&self.root).map_err(|e| io_error(&e, "creating the folder"))?;
        let probe = self
            .root
            .join(format!(".fathom-write-test-{}", std::process::id()));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
            .map_err(|e| io_error(&e, "writing to the folder"))?;
        std::fs::remove_file(&probe).map_err(|e| io_error(&e, "cleaning up"))
    }
}

#[async_trait]
impl ExportDestination for FolderDestination {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn describe(&self) -> String {
        self.root.display().to_string()
    }

    async fn test(&self) -> Result<(), ExportError> {
        let this = Self::new(self.kind, self.root.clone(), self.must_exist);
        tokio::task::spawn_blocking(move || this.test_blocking())
            .await
            .map_err(|e| ExportError::Other(e.to_string()))?
    }

    async fn put(&self, path: &str, data: Vec<u8>) -> Result<String, ExportError> {
        let this = Self::new(self.kind, self.root.clone(), self.must_exist);
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || this.put_blocking(&path, &data))
            .await
            .map_err(|e| ExportError::Other(e.to_string()))?
    }
}

/// Builds folder destinations. `local` creates the folder when needed; `smb` and `nfs` require
/// it to exist (the mount point). Settings: `path` (or `mount_path`).
pub struct FolderFactory {
    kind: &'static str,
    must_exist: bool,
}

impl FolderFactory {
    pub fn local() -> Self {
        Self {
            kind: "local",
            must_exist: false,
        }
    }

    pub fn smb() -> Self {
        Self {
            kind: "smb",
            must_exist: true,
        }
    }

    pub fn nfs() -> Self {
        Self {
            kind: "nfs",
            must_exist: true,
        }
    }
}

impl DestinationFactory for FolderFactory {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn create(
        &self,
        config: &DestinationConfig,
    ) -> Result<Arc<dyn ExportDestination>, ExportError> {
        let path = config
            .settings
            .get("path")
            .or_else(|| config.settings.get("mount_path"))
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .ok_or_else(|| {
                ExportError::Invalid(format!(
                    "`{}`: set `path` to the folder{}.",
                    config.name,
                    if self.must_exist {
                        " where the share is mounted"
                    } else {
                        ""
                    }
                ))
            })?;
        Ok(Arc::new(FolderDestination::new(
            self.kind,
            PathBuf::from(path),
            self.must_exist,
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn dest(dir: &Path, must_exist: bool) -> FolderDestination {
        FolderDestination::new("local", dir.to_path_buf(), must_exist)
    }

    #[tokio::test]
    async fn files_are_written_under_the_root_creating_folders() {
        let dir = tempfile::tempdir().unwrap();
        let d = dest(dir.path(), false);
        let at = d
            .put("exports/2026/customers.csv", b"id\n1\n".to_vec())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&at).unwrap(), b"id\n1\n");
        assert!(at.starts_with(dir.path().to_str().unwrap()));
    }

    #[tokio::test]
    async fn an_existing_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let d = dest(dir.path(), false);
        d.put("a.csv", b"one".to_vec()).await.unwrap();
        let again = d.put("a.csv", b"two".to_vec()).await;
        assert!(matches!(again, Err(ExportError::AlreadyExists(_))));
        assert_eq!(std::fs::read(dir.path().join("a.csv")).unwrap(), b"one");
    }

    #[tokio::test]
    async fn paths_cannot_leave_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let d = dest(&dir.path().join("root"), false);
        for bad in [
            "../escape.csv",
            "a/../../escape.csv",
            "/abs.csv",
            "",
            "a/\0b",
            "..",
        ] {
            assert!(d.put(bad, vec![1]).await.is_err(), "{bad:?}");
        }
        assert!(!dir.path().join("escape.csv").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_inside_the_root_cannot_lead_outside() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let d = dest(&root, false);
        let r = d.put("link/stolen.csv", vec![1]).await;
        assert!(matches!(r, Err(ExportError::Invalid(_))), "{r:?}");
        assert!(!outside.join("stolen.csv").exists());
    }

    #[tokio::test]
    async fn mounted_kinds_refuse_a_missing_mount_point_and_never_create_it() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-mounted");
        let d = FolderDestination::new("smb", missing.clone(), true);
        let r = d.put("a.csv", vec![1]).await;
        assert!(matches!(r, Err(ExportError::Unreachable(m)) if m.contains("mounted")));
        assert!(d.test().await.is_err());
        assert!(
            !missing.exists(),
            "a missing share must not turn into a local folder"
        );
    }

    #[tokio::test]
    async fn test_checks_writability_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let d = dest(dir.path(), true);
        d.test().await.unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn factories_need_a_path_and_accept_mount_path_for_shares() {
        let cfg = |settings: &[(&str, &str)]| DestinationConfig {
            name: "Team share".into(),
            kind: "smb".into(),
            settings: settings
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<BTreeMap<_, _>>(),
            ..DestinationConfig::default()
        };
        let err = FolderFactory::smb().create(&cfg(&[])).err().unwrap();
        assert!(err.to_string().contains("mounted"), "{err}");
        let d = FolderFactory::smb()
            .create(&cfg(&[("mount_path", "/mnt/team")]))
            .unwrap();
        assert_eq!((d.kind(), d.describe().as_str()), ("smb", "/mnt/team"));
        assert!(
            FolderFactory::local()
                .create(&cfg(&[("path", "  ")]))
                .is_err()
        );
    }
}
