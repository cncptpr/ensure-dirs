//! Configuration types.
//!
//! This is exactly the JSON produced by `builtins.toJSON config.ensureDirs`
//! in the NixOS module: camelCase keys, directory paths as map keys, and an
//! optional `mode` per directory (null when unset).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_use_cache")]
    pub use_cache: bool,
    #[serde(default = "default_cache_dir")]
    pub cache_dir: String,
    #[serde(default)]
    pub dirs: BTreeMap<String, DirSpec>,
}

fn default_use_cache() -> bool {
    true
}

fn default_cache_dir() -> String {
    "/var/cache/ensure-dirs".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirSpec {
    pub user: String,
    pub group: String,
    #[serde(default)]
    pub mode: Option<String>,
}

/// Parse an octal permission string such as "0755", "755" or "0o755".
pub fn parse_mode(s: &str) -> Result<u32, String> {
    let t = s.trim();
    let t = t
        .strip_prefix("0o")
        .or_else(|| t.strip_prefix("0O"))
        .unwrap_or(t);
    if t.is_empty() {
        return Err("mode is empty".to_string());
    }
    let value =
        u32::from_str_radix(t, 8).map_err(|_| format!("mode '{s}' is not a valid octal number"))?;
    if value > 0o7777 {
        return Err(format!("mode '{s}' is out of range"));
    }
    Ok(value)
}

/// Canonical representation of a mode: "0755".
pub fn format_mode(value: u32) -> String {
    format!("{value:04o}")
}

impl DirSpec {
    /// Validate and canonicalize this entry (mode becomes a canonical
    /// octal string so that equal modes compare equal).
    pub fn normalize(&self, path: &str) -> Result<DirSpec, String> {
        let mut errors = Vec::new();
        if self.user.is_empty() {
            errors.push("user must not be empty".to_string());
        }
        if self.group.is_empty() {
            errors.push("group must not be empty".to_string());
        }
        let mode = match &self.mode {
            None => None,
            Some(m) => match parse_mode(m) {
                Ok(v) => Some(format_mode(v)),
                Err(e) => {
                    errors.push(e);
                    None
                }
            },
        };
        if !errors.is_empty() {
            return Err(format!("{path}: {}", errors.join("; ")));
        }
        Ok(DirSpec {
            user: self.user.clone(),
            group: self.group.clone(),
            mode,
        })
    }
}

/// Validate the whole configuration: absolute paths, non-empty user/group,
/// well-formed modes. Returns the normalized configuration.
pub fn validate(config: Config) -> Result<Config, String> {
    let mut errors = Vec::new();
    let mut dirs = BTreeMap::new();
    for (path, spec) in &config.dirs {
        if !path.starts_with('/') {
            errors.push(format!("{path}: path must be absolute"));
            continue;
        }
        match spec.normalize(path) {
            Ok(s) => {
                dirs.insert(path.clone(), s);
            }
            Err(e) => errors.push(e),
        }
    }
    if config.cache_dir.trim().is_empty() {
        errors.push("cacheDir must not be empty".to_string());
    }
    if !errors.is_empty() {
        return Err(format!("invalid configuration:\n  {}", errors.join("\n  ")));
    }
    Ok(Config { dirs, ..config })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(user: &str, group: &str, mode: Option<&str>) -> DirSpec {
        DirSpec {
            user: user.to_string(),
            group: group.to_string(),
            mode: mode.map(str::to_string),
        }
    }

    #[test]
    fn parses_octal_modes() {
        assert_eq!(parse_mode("0755").unwrap(), 0o755);
        assert_eq!(parse_mode("755").unwrap(), 0o755);
        assert_eq!(parse_mode("0o755").unwrap(), 0o755);
        assert_eq!(parse_mode("7777").unwrap(), 0o7777);
        assert_eq!(parse_mode("0000").unwrap(), 0);
    }

    #[test]
    fn rejects_bad_modes() {
        assert!(parse_mode("").is_err());
        assert!(parse_mode("8").is_err());
        assert!(parse_mode("abc").is_err());
        assert!(parse_mode("100000").is_err());
        assert!(parse_mode("-755").is_err());
    }

    #[test]
    fn normalizes_mode_canonically() {
        let a = spec("alice", "users", Some("755"))
            .normalize("/srv/a")
            .unwrap();
        let b = spec("alice", "users", Some("0755"))
            .normalize("/srv/a")
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(a.mode.as_deref(), Some("0755"));
    }

    #[test]
    fn rejects_relative_paths_and_empty_users() {
        let cfg = Config {
            use_cache: true,
            cache_dir: "/var/cache/ensure-dirs".to_string(),
            dirs: BTreeMap::from([
                ("relative/path".to_string(), spec("alice", "users", None)),
                ("/srv/b".to_string(), spec("", "users", None)),
            ]),
        };
        let err = validate(cfg).unwrap_err();
        assert!(err.contains("must be absolute"), "{err}");
        assert!(err.contains("user must not be empty"), "{err}");
    }

    #[test]
    fn parses_nix_style_json() {
        // Exactly what builtins.toJSON produces, including "mode": null.
        let json = r#"
            {"useCache":true,
             "cacheDir":"/var/cache/ensure-dirs",
             "dirs":{
               "/srv/data":{"user":"alice","group":"users","mode":"0750"},
               "/srv/misc":{"user":"bob","group":"users","mode":null}}}
        "#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        let cfg = validate(cfg).unwrap();
        assert!(cfg.use_cache);
        assert_eq!(cfg.dirs.len(), 2);
        assert_eq!(cfg.dirs["/srv/data"].mode.as_deref(), Some("0750"));
        assert_eq!(cfg.dirs["/srv/misc"].mode, None);
    }

    #[test]
    fn round_trips_through_json() {
        let cfg = Config {
            use_cache: false,
            cache_dir: "/tmp/cache".to_string(),
            dirs: BTreeMap::from([("/srv/a".to_string(), spec("alice", "users", Some("0700")))]),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"useCache\":false"), "{json}");
        assert!(json.contains("\"cacheDir\":\"/tmp/cache\""), "{json}");
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
