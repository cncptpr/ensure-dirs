//! Filesystem operations and user/group lookups.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

/// Whether we are running with euid 0.
pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// Resolve a user/group name to (uid, gid). NSS-aware (uzers -> libc).
pub fn lookup_ids(user: &str, group: &str) -> Result<(u32, u32), String> {
    let uid = uzers::get_user_by_name(user)
        .map(|u| u.uid())
        .ok_or_else(|| format!("user '{user}' does not exist"))?;
    let gid = uzers::get_group_by_name(group)
        .map(|g| g.gid())
        .ok_or_else(|| format!("group '{group}' does not exist"))?;
    Ok((uid, gid))
}

fn chown(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    let c = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;
    if unsafe { libc::chown(c.as_ptr(), uid, gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Ensure `path` is a directory owned by `uid:gid`, with `mode` if given.
///
/// Performs the minimum number of syscalls: chown and chmod are only
/// issued when the current state does not already match.
pub fn ensure_dir(path: &str, uid: u32, gid: u32, mode: Option<u32>) -> Result<(), String> {
    let p = Path::new(path);

    if let Err(e) = fs::create_dir_all(p) {
        let why = match fs::symlink_metadata(p) {
            Ok(md) if md.file_type().is_symlink() => match fs::metadata(p) {
                Ok(target) if target.is_dir() => format!("failed to create directory: {e}"),
                Ok(_) => "is a symlink to a non-directory".to_string(),
                Err(_) => format!("is a symlink with a missing or inaccessible target: {e}"),
            },
            Ok(md) if md.is_dir() => format!("failed to create directory: {e}"),
            Ok(_) => "exists but is not a directory".to_string(),
            Err(_) => format!("failed to create directory: {e}"),
        };
        return Err(why);
    }

    let md = fs::metadata(p).map_err(|e| format!("cannot stat: {e}"))?;
    if !md.is_dir() {
        return Err("exists but is not a directory".to_string());
    }

    if md.uid() != uid || md.gid() != gid {
        chown(p, uid, gid).map_err(|e| format!("cannot chown to {uid}:{gid}: {e}"))?;
    }

    if let Some(m) = mode {
        let current = md.permissions().mode() & 0o7777;
        if current != m {
            fs::set_permissions(p, fs::Permissions::from_mode(m))
                .map_err(|e| format!("cannot chmod to {m:04o}: {e}"))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ensure-dirs-ops-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn creates_directory_tree_with_mode() {
        let root = tmpdir("create");
        let target = root.join("a/b/c");
        let (uid, gid) = (unsafe { libc::geteuid() }, unsafe { libc::getegid() });
        ensure_dir(target.to_str().unwrap(), uid, gid, Some(0o750)).unwrap();
        let md = fs::metadata(&target).unwrap();
        assert!(md.is_dir());
        assert_eq!(md.permissions().mode() & 0o7777, 0o750);
        assert_eq!(md.uid(), uid);
        assert_eq!(md.gid(), gid);
        // Second run is a no-op (already matching).
        ensure_dir(target.to_str().unwrap(), uid, gid, Some(0o750)).unwrap();
        // No mode requested: permissions untouched.
        ensure_dir(target.to_str().unwrap(), uid, gid, None).unwrap();
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
            0o750
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn existing_file_is_an_error() {
        let root = tmpdir("file");
        let file = root.join("file");
        fs::write(&file, "x").unwrap();
        let (uid, gid) = (unsafe { libc::geteuid() }, unsafe { libc::getegid() });
        let err = ensure_dir(file.to_str().unwrap(), uid, gid, None).unwrap_err();
        assert!(err.contains("not a directory"), "{err}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn dangling_symlink_is_an_error() {
        let root = tmpdir("symlink");
        let link = root.join("link");
        std::os::unix::fs::symlink(root.join("missing"), &link).unwrap();
        let (uid, gid) = (unsafe { libc::geteuid() }, unsafe { libc::getegid() });
        let err = ensure_dir(link.to_str().unwrap(), uid, gid, None).unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn chown_failure_is_reported() {
        let root = tmpdir("chown");
        let target = root.join("owned");
        fs::create_dir(&target).unwrap();
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        // Unprivileged processes cannot chown to another user: the error
        // must surface instead of being swallowed.
        let err = ensure_dir(target.to_str().unwrap(), uid + 1, gid, None).unwrap_err();
        assert!(err.contains("cannot chown"), "{err}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn lookup_fails_for_unknown_names() {
        let err = lookup_ids("no-such-user-ensure-dirs", "no-such-group-ensure-dirs").unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
    }

    #[test]
    fn lookup_resolves_current_user() {
        let username = uzers::get_current_username().unwrap();
        let groupname = uzers::get_current_groupname().unwrap();
        let (uid, gid) =
            lookup_ids(username.to_str().unwrap(), groupname.to_str().unwrap()).unwrap();
        assert_eq!(uid, uzers::get_current_uid());
        assert_eq!(gid, uzers::get_current_gid());
    }
}
