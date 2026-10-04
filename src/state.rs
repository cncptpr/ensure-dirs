//! Cached state (the last applied configuration) and the diff planner.

use crate::config::DirSpec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

pub const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    #[serde(default)]
    pub dirs: BTreeMap<String, DirSpec>,
}

impl State {
    pub fn empty() -> Self {
        State {
            version: STATE_VERSION,
            dirs: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Identical to the cached state: no filesystem interaction at all.
    Skip,
    /// New or changed: create, check, chown/chmod as needed.
    Ensure,
}

#[derive(Debug, Clone)]
pub struct Work {
    pub path: String,
    pub spec: DirSpec,
    pub action: Action,
}

/// Load the cached state. `Ok(None)` means there is no cache file yet.
pub fn load(path: &Path) -> Result<Option<State>, String> {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let state: State = serde_json::from_slice(&data)
        .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
    if state.version != STATE_VERSION {
        return Err(format!(
            "{} has unsupported version {} (expected {STATE_VERSION})",
            path.display(),
            state.version
        ));
    }
    Ok(Some(state))
}

/// Persist the state atomically: create the directory, write a temp file,
/// fsync, chmod 0600, rename into place.
pub fn store(path: &Path, state: &State) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let json =
        serde_json::to_string_pretty(state).map_err(|e| format!("cannot serialize state: {e}"))?;

    let tmp = dir.join(format!("state.json.tmp.{}", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot write {}: {e}", tmp.display()));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "cannot rename {} -> {}: {e}",
            tmp.display(),
            path.display()
        ));
    }
    Ok(())
}

/// Diff the desired configuration against the cached state.
///
/// - present in both and identical -> `Skip` (zero filesystem interaction)
/// - new or changed -> `Ensure`
/// - removed from the configuration -> not part of the plan at all
///   (directories are never deleted; `ensureDirs` is not a dir manager)
pub fn plan(current: &BTreeMap<String, DirSpec>, previous: &State) -> Vec<Work> {
    current
        .iter()
        .map(|(path, spec)| Work {
            path: path.clone(),
            spec: spec.clone(),
            action: if previous.dirs.get(path) == Some(spec) {
                Action::Skip
            } else {
                Action::Ensure
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn spec(user: &str, group: &str, mode: Option<&str>) -> DirSpec {
        DirSpec {
            user: user.to_string(),
            group: group.to_string(),
            mode: mode.map(str::to_string),
        }
    }

    fn dirs(entries: &[(&str, DirSpec)]) -> BTreeMap<String, DirSpec> {
        entries
            .iter()
            .map(|(p, s)| (p.to_string(), s.clone()))
            .collect()
    }

    fn actions(current: &BTreeMap<String, DirSpec>, previous: &State) -> Vec<(String, Action)> {
        plan(current, previous)
            .into_iter()
            .map(|w| (w.path, w.action))
            .collect()
    }

    #[test]
    fn unchanged_entries_are_skipped() {
        let previous = State {
            version: STATE_VERSION,
            dirs: dirs(&[
                ("/srv/a", spec("alice", "users", Some("0755"))),
                ("/srv/b", spec("bob", "users", None)),
            ]),
        };
        let current = dirs(&[
            ("/srv/a", spec("alice", "users", Some("0755"))),
            ("/srv/b", spec("bob", "users", None)),
        ]);
        assert!(actions(&current, &previous)
            .iter()
            .all(|(_, a)| *a == Action::Skip));
    }

    #[test]
    fn new_changed_and_removed_entries() {
        let previous = State {
            version: STATE_VERSION,
            dirs: dirs(&[
                ("/srv/a", spec("alice", "users", Some("0755"))),
                ("/srv/removed", spec("alice", "users", None)),
            ]),
        };
        let current = dirs(&[
            // unchanged
            ("/srv/a", spec("alice", "users", Some("0755"))),
            // changed owner
            ("/srv/b", spec("carol", "users", None)),
            // new
            ("/srv/c", spec("dave", "users", Some("0700"))),
        ]);
        let plan = actions(&current, &previous);
        assert_eq!(
            plan,
            vec![
                ("/srv/a".to_string(), Action::Skip),
                ("/srv/b".to_string(), Action::Ensure),
                ("/srv/c".to_string(), Action::Ensure),
            ]
        );
        // The removed directory must not appear anywhere in the plan.
        assert!(plan.iter().all(|(p, _)| p != "/srv/removed"));
    }

    #[test]
    fn mode_change_triggers_ensure() {
        let previous = State {
            version: STATE_VERSION,
            dirs: dirs(&[("/srv/a", spec("alice", "users", Some("0755")))]),
        };
        let current = dirs(&[("/srv/a", spec("alice", "users", Some("0700")))]);
        assert_eq!(actions(&current, &previous)[0].1, Action::Ensure);
    }

    #[test]
    fn empty_previous_ensures_everything() {
        let current = dirs(&[
            ("/srv/a", spec("alice", "users", None)),
            ("/srv/b", spec("bob", "users", None)),
        ]);
        assert!(actions(&current, &State::empty())
            .iter()
            .all(|(_, a)| *a == Action::Ensure));
    }

    #[test]
    fn store_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("ensure-dirs-test-{}", std::process::id()));
        let path: PathBuf = dir.join("state.json");
        let state = State {
            version: STATE_VERSION,
            dirs: dirs(&[("/srv/a", spec("alice", "users", Some("0755")))]),
        };
        store(&path, &state).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o600);
        let back = load(&path).unwrap().unwrap();
        assert_eq!(back, state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_missing_and_corrupt_cache() {
        let dir = std::env::temp_dir().join(format!("ensure-dirs-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        assert!(load(&path).unwrap().is_none());
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load(&path).unwrap_err().contains("not valid JSON"));
        std::fs::write(&path, r#"{"version":99,"dirs":{}}"#).unwrap();
        assert!(load(&path).unwrap_err().contains("unsupported version"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
