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
            std::fs::create_dir_all(backups).map_err(|e| format!("could not create the backup folder: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(backups, std::fs::Permissions::from_mode(0o700))
                    .map_err(|e| format!("could not make the backup folder private: {e}"))?;
            }
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let name = plan.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let random = crate::ui::random_hex().map_err(|e| format!("secure randomness is unavailable: {e}"))?;
            let b = backups.join(format!("{secs}-{}-{name}", &random[..8]));
            let mut backup_file = create_new(&b, 0o600).map_err(|e| format!("could not save a backup: {e}"))?;
            backup_file.write_all(text.as_bytes()).map_err(|e| format!("could not save a backup: {e}"))?;
            backup_file.sync_all().map_err(|e| format!("could not save a backup: {e}"))?;
            Some(b)
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
