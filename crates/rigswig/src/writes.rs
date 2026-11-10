//! Write mode, the applying half: only `rigswig ui --allow-writes` uses it. A plan from rigswig-core is applied
//! atomically (a temporary file in the same folder, then a rename), after a dated backup of what was there, and only
//! if the file is unchanged since the plan was made. Undo puts the backup back, only if the file is still as we left
//! it.

use std::io::Write;
use std::path::{Path, PathBuf};

use rigswig_core::change::Plan;
use rigswig_core::read::redact::sha256_hex;

/// What undo needs: the file, what it held before (None: it did not exist), and what we wrote.
#[derive(Debug, Clone)]
pub struct Applied {
    pub path: PathBuf,
    pub file: String,
    pub before: Option<String>,
    pub after_hash: String,
    pub backup: Option<PathBuf>,
}

fn current(path: &Path) -> Result<Option<String>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            Err("that settings file is a link; rigswig does not write through links".into())
        }
        Ok(_) => std::fs::read_to_string(path).map(Some).map_err(|_| "that settings file cannot be read".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("that settings file cannot be read".into()),
    }
}

/// Create a file which did not exist before. On Unix, `mode` is applied at creation, before any contents are written.
fn create_new(path: &Path, _mode: u32) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(_mode);
    }
    options.open(path)
}

/// Save one backup without following a backup-directory symlink. Unix uses directory descriptors for every component,
/// so a link swapped in between a metadata check and creation cannot redirect the write.
#[cfg(unix)]
mod backup {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    fn cstring(value: &std::ffi::OsStr) -> Result<std::ffi::CString, String> {
        std::ffi::CString::new(value.as_bytes()).map_err(|_| "backup path contains a NUL byte".into())
    }

    fn open_dir(path: &Path) -> Result<OwnedFd, String> {
        let path = cstring(path.as_os_str())?;
        let raw = unsafe {
            libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        };
        if raw < 0 {
            return Err(format!("could not open the backup folder: {}", std::io::Error::last_os_error()));
        }
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    fn open_child(dir: &OwnedFd, name: &std::ffi::OsStr) -> Result<OwnedFd, String> {
        let name = cstring(name)?;
        let raw = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if raw < 0 {
            return Err(format!("could not open the backup folder: {}", std::io::Error::last_os_error()));
        }
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    fn private_dir(path: &Path) -> Result<OwnedFd, String> {
        if !path.is_absolute() {
            return Err("the backup folder must be an absolute path".into());
        }
        let mut ancestor = path;
        let mut missing = Vec::new();
        loop {
            match std::fs::symlink_metadata(ancestor) {
                Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                    return Err("the backup folder and its parents must be real directories, not links".into());
                }
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(ancestor.file_name().ok_or("invalid backup folder")?.to_os_string());
                    ancestor = ancestor.parent().ok_or("invalid backup folder")?;
                }
                Err(e) => return Err(format!("could not inspect the backup folder: {e}")),
            }
        }
        let mut dir = open_dir(ancestor)?;
        for name in missing.iter().rev() {
            let c_name = cstring(name)?;
            if unsafe { libc::mkdirat(dir.as_raw_fd(), c_name.as_ptr(), 0o700) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(format!("could not create the backup folder: {error}"));
                }
            }
            dir = open_child(&dir, name)?;
        }
        if unsafe { libc::fchmod(dir.as_raw_fd(), 0o700) } != 0 {
            return Err(format!("could not make the backup folder private: {}", std::io::Error::last_os_error()));
        }
        Ok(dir)
    }

    pub fn save(root: &Path, name: &std::ffi::OsStr, text: &str) -> Result<PathBuf, String> {
        let dir = private_dir(root)?;
        let file_name = name.to_os_string();
        let name = cstring(name)?;
        let raw = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if raw < 0 {
            return Err(format!("could not save a backup: {}", std::io::Error::last_os_error()));
        }
        let mut file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(raw) });
        file.write_all(text.as_bytes()).map_err(|e| format!("could not save a backup: {e}"))?;
        file.sync_all().map_err(|e| format!("could not save a backup: {e}"))?;
        Ok(root.join(file_name))
    }
}

/// The portable fallback checks every existing and newly created component immediately before `create_new` (which
/// refuses an existing final link). Windows has no std equivalent of Unix's openat/O_NOFOLLOW directory traversal.
#[cfg(not(unix))]
mod backup {
    use super::*;

    fn private_dir(path: &Path) -> Result<(), String> {
        let mut current = PathBuf::new();
        for component in path.components() {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                    return Err("the backup folder and its parents must be real directories, not links".into());
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir(&current).map_err(|e| format!("could not create the backup folder: {e}"))?;
                    let meta = std::fs::symlink_metadata(&current)
                        .map_err(|e| format!("could not inspect the backup folder: {e}"))?;
                    if meta.file_type().is_symlink() || !meta.is_dir() {
                        return Err("the backup folder and its parents must be real directories, not links".into());
                    }
                }
                Err(e) => return Err(format!("could not inspect the backup folder: {e}")),
            }
        }
        Ok(())
    }

    pub fn save(root: &Path, name: &std::ffi::OsStr, text: &str) -> Result<PathBuf, String> {
        private_dir(root)?;
        let path = root.join(name);
        let mut file = create_new(&path, 0o600).map_err(|e| format!("could not save a backup: {e}"))?;
        file.write_all(text.as_bytes()).map_err(|e| format!("could not save a backup: {e}"))?;
        file.sync_all().map_err(|e| format!("could not save a backup: {e}"))?;
        Ok(path)
    }
}

/// Write `text` to `path` atomically: a temporary file beside it, flushed, then renamed over it. An existing file's
/// permissions survive the replacement; a new settings file is owner-only on Unix.
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("no folder for that file")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let permissions = std::fs::symlink_metadata(path).ok().map(|m| m.permissions());
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        permissions.as_ref().map_or(0o600, |p| p.mode())
    };
    #[cfg(not(unix))]
    let mode = 0;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let random = crate::ui::random_hex().map_err(|e| format!("secure randomness is unavailable: {e}"))?;
    let tmp = dir.join(format!(".{name}.rigswig-{random}.tmp"));
    let result = (|| {
        let mut f = create_new(&tmp, mode)?;
        f.write_all(text.as_bytes())?;
        if let Some(p) = permissions {
            f.set_permissions(p)?;
        }
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// Apply `plan`. `before_hash` is what the page saw; the file must still match it.
pub fn apply(plan: &Plan, before_hash: Option<&str>, backups: &Path) -> Result<Applied, String> {
    let now = current(&plan.path)?;
    let now_hash = now.as_deref().map(sha256_hex);
    if now_hash.as_deref() != before_hash || now_hash != plan.before_hash {
        return Err("the file changed since you looked at it; review the change again".into());
    }
    let backup = match &now {
        Some(text) => {
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let name = plan.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let random = crate::ui::random_hex().map_err(|e| format!("secure randomness is unavailable: {e}"))?;
            let name = std::ffi::OsString::from(format!("{secs}-{}-{name}", &random[..8]));
            Some(backup::save(backups, &name, text)?)
        }
        None => None,
    };
    write_atomic(&plan.path, &plan.after)?;
    Ok(Applied {
        path: plan.path.clone(),
        file: plan.file.clone(),
        before: now,
        after_hash: sha256_hex(&plan.after),
        backup,
    })
}

/// Undo `a`: put back what was there (or remove the file we created), if the file is still exactly what we wrote.
pub fn undo(a: &Applied) -> Result<(), String> {
    let now = current(&a.path)?;
    if now.as_deref().map(sha256_hex).as_deref() != Some(a.after_hash.as_str()) {
        return Err(format!("{} changed since the change was applied; restore it by hand from the backup", a.file));
    }
    match &a.before {
        Some(text) => write_atomic(&a.path, text),
        None => std::fs::remove_file(&a.path).map_err(|e| format!("could not remove {}: {e}", a.path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rigswig_core::change::{Change, Scope};

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rigswig-writes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn plan(path: &Path, before: Option<&str>, after: &str) -> Plan {
        Plan {
            change: Change::Plugin { id: "a@b".into(), scope: Scope::Local, on: false },
            file: "settings.local.json".into(),
            path: path.to_path_buf(),
            before_hash: before.map(sha256_hex),
            after: after.into(),
            creates_file: before.is_none(),
            diff: Vec::new(),
            tokens_before: 0,
            tokens_after: 0,
            takes_effect: true,
            notes: Vec::new(),
        }
    }

    #[test]
    fn apply_backs_up_and_undo_restores() {
        let d = dir("roundtrip");
        let f = d.join(".claude/settings.json");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, "old\n").unwrap();
        let p = plan(&f, Some("old\n"), "new\n");
        let a = apply(&p, p.before_hash.as_deref(), &d.join("backups")).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "new\n");
        assert_eq!(std::fs::read_to_string(a.backup.as_ref().unwrap()).unwrap(), "old\n");
        assert!(
            std::fs::read_dir(f.parent().unwrap()).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp"))
        );
        undo(&a).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "old\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_created_file_is_removed_by_undo() {
        let d = dir("create");
        let f = d.join(".claude/settings.local.json");
        let p = plan(&f, None, "{}\n");
        let a = apply(&p, None, &d.join("backups")).unwrap();
        assert!(a.backup.is_none());
        undo(&a).unwrap();
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replacement_preserves_the_settings_files_permissions() {
        let d = dir("permissions");
        let f = d.join("settings.json");
        std::fs::write(&f, "old\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        let p = plan(&f, Some("old\n"), "new\n");
        apply(&p, p.before_hash.as_deref(), &d.join("backups")).unwrap();
        assert_eq!(mode(&f), 0o600, "atomic replacement must not broaden the settings file");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn backups_are_private() {
        let d = dir("private-backup");
        let f = d.join("settings.json");
        std::fs::write(&f, "old\n").unwrap();
        let backups = d.join("backups");
        let p = plan(&f, Some("old\n"), "new\n");
        let a = apply(&p, p.before_hash.as_deref(), &backups).unwrap();
        assert_eq!(mode(&backups), 0o700);
        assert_eq!(mode(a.backup.as_ref().unwrap()), 0o600);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_backup_paths_are_refused_without_touching_their_targets() {
        for parent_link in [false, true] {
            let d = dir(if parent_link { "backup-parent-link" } else { "backup-dir-link" });
            let f = d.join("settings.json");
            std::fs::write(&f, "old\n").unwrap();
            let outside = d.join("outside");
            std::fs::create_dir(&outside).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o755)).unwrap();
            let backups = if parent_link {
                let parent = d.join("container");
                std::os::unix::fs::symlink(&outside, &parent).unwrap();
                parent.join("backups")
            } else {
                let backups = d.join("backups");
                std::os::unix::fs::symlink(&outside, &backups).unwrap();
                backups
            };
            let p = plan(&f, Some("old\n"), "new\n");
            let error = apply(&p, p.before_hash.as_deref(), &backups).unwrap_err();
            assert!(error.contains("backup"), "{error}");
            assert_eq!(std::fs::read_to_string(&f).unwrap(), "old\n", "the settings write never happens");
            assert_eq!(mode(&outside), 0o755, "chmod never follows a backup link");
            assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "no backup lands outside");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_created_settings_file_is_owner_only() {
        let d = dir("new-permissions");
        let f = d.join("settings.json");
        let p = plan(&f, None, "{}\n");
        apply(&p, None, &d.join("backups")).unwrap();
        assert_eq!(mode(&f), 0o600);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn stale_plans_links_and_edited_files_are_refused() {
        let d = dir("refuse");
        let f = d.join("s.json");
        std::fs::write(&f, "old\n").unwrap();
        let p = plan(&f, Some("old\n"), "new\n");
        std::fs::write(&f, "someone else\n").unwrap();
        assert!(apply(&p, p.before_hash.as_deref(), &d.join("b")).unwrap_err().contains("changed since"));
        std::fs::write(&f, "old\n").unwrap();
        assert!(
            apply(&p, Some("not-the-hash"), &d.join("b")).unwrap_err().contains("changed since"),
            "the page's hash is checked too"
        );

        let a = apply(&p, p.before_hash.as_deref(), &d.join("b")).unwrap();
        std::fs::write(&f, "edited after\n").unwrap();
        assert!(undo(&a).unwrap_err().contains("by hand"));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "edited after\n", "undo never overwrites someone else's edit");

        #[cfg(unix)]
        {
            let target = d.join("target.json");
            std::fs::write(&target, "t\n").unwrap();
            let link = d.join("link.json");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let lp = plan(&link, Some("t\n"), "x\n");
            assert!(apply(&lp, lp.before_hash.as_deref(), &d.join("b")).unwrap_err().contains("link"));
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "t\n");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
