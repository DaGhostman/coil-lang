//! `coil.lock` — locked dependency versions and commits.

use std::collections::BTreeMap;
use std::path::Path;

pub const LOCKFILE_NAME: &str = "coil.lock";
pub const LOCKFILE_VERSION: u32 = 1;

#[derive(Debug)]
pub struct LockError(pub String);

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for LockError {}

/// One locked package entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockPackage {
    pub git: String,
    /// Resolved concrete semver (no range).
    pub version: String,
    /// Exact git commit SHA locked at install/update time.
    pub commit: String,
    pub path: Option<String>,
}

/// Parsed `coil.lock`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lockfile {
    pub packages: BTreeMap<String, LockPackage>,
}

impl Lockfile {
    pub fn load(path: &Path) -> Result<Self, LockError> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| LockError(format!("failed to read `{}`: {e}", path.display())))?;
        Self::parse(&contents)
    }

    pub fn parse(source: &str) -> Result<Self, LockError> {
        let mut packages: BTreeMap<String, LockPackage> = BTreeMap::new();
        let mut version: Option<u32> = None;
        let mut current: Option<(String, Pending)> = None;

        for (idx, raw) in source.lines().enumerate() {
            let line_num = idx + 1;
            let line = match strip_comment(raw) {
                Some(l) => l.trim(),
                None => continue,
            };
            if line.is_empty() {
                continue;
            }

            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                if let Some((pkg_name, pending)) = current.take() {
                    packages.insert(pkg_name, pending.into_pkg(line_num)?);
                }
                let name = name.trim();
                if let Some(rest) = name.strip_prefix("package.") {
                    if rest.is_empty() {
                        return Err(LockError(format!(
                            "lock parse error at line {line_num}: empty package name"
                        )));
                    }
                    current = Some((rest.to_string(), Pending::default()));
                } else {
                    return Err(LockError(format!(
                        "lock parse error at line {line_num}: unknown section `[{name}]`"
                    )));
                }
                continue;
            }

            let (key, value) = split_kv(line).ok_or_else(|| {
                LockError(format!(
                    "lock parse error at line {line_num}: expected `key = value`"
                ))
            })?;

            if current.is_none() && key == "version" {
                let n: u32 = if let Some(s) = parse_string(value) {
                    s.parse::<u32>().map_err(|_| {
                        LockError(format!(
                            "lock parse error at line {line_num}: invalid lock version"
                        ))
                    })?
                } else {
                    value.trim().parse::<u32>().map_err(|_| {
                        LockError(format!(
                            "lock parse error at line {line_num}: invalid lock version"
                        ))
                    })?
                };
                version = Some(n);
                continue;
            }

            let (_, pending) = current.as_mut().ok_or_else(|| {
                LockError(format!(
                    "lock parse error at line {line_num}: key before `[package.*]`"
                ))
            })?;
            let parsed = parse_string(value).ok_or_else(|| {
                LockError(format!(
                    "lock parse error at line {line_num}: expected string"
                ))
            })?;
            match key {
                "git" => pending.git = Some(parsed),
                "version" => pending.version = Some(parsed),
                "commit" => pending.commit = Some(parsed),
                "path" => pending.path = Some(parsed),
                other => {
                    return Err(LockError(format!(
                        "lock parse error at line {line_num}: unknown key `{other}`"
                    )));
                }
            }
        }

        if let Some((pkg_name, pending)) = current.take() {
            packages.insert(pkg_name, pending.into_pkg(source.lines().count().max(1))?);
        }

        match version {
            None | Some(LOCKFILE_VERSION) => {}
            Some(v) => {
                return Err(LockError(format!(
                    "unsupported coil.lock version {v} (expected {LOCKFILE_VERSION})"
                )));
            }
        }

        Ok(Self { packages })
    }

    pub fn save(&self, path: &Path) -> Result<(), LockError> {
        let mut out = String::new();
        out.push_str("# This file is automatically @generated by coil.\n");
        out.push_str("# It is not intended for manual editing.\n");
        out.push_str(&format!("version = {LOCKFILE_VERSION}\n"));
        for (name, pkg) in &self.packages {
            out.push('\n');
            out.push_str(&format!("[package.{name}]\n"));
            out.push_str(&format!("git = \"{}\"\n", pkg.git));
            out.push_str(&format!("version = \"{}\"\n", pkg.version));
            out.push_str(&format!("commit = \"{}\"\n", pkg.commit));
            if let Some(p) = &pkg.path {
                out.push_str(&format!("path = \"{p}\"\n"));
            }
        }
        std::fs::write(path, out)
            .map_err(|e| LockError(format!("failed to write `{}`: {e}", path.display())))?;
        Ok(())
    }
}

#[derive(Default)]
struct Pending {
    git: Option<String>,
    version: Option<String>,
    commit: Option<String>,
    path: Option<String>,
}

impl Pending {
    fn into_pkg(self, line: usize) -> Result<LockPackage, LockError> {
        Ok(LockPackage {
            git: self.git.ok_or_else(|| {
                LockError(format!("lock entry missing `git` (near line {line})"))
            })?,
            version: self.version.ok_or_else(|| {
                LockError(format!("lock entry missing `version` (near line {line})"))
            })?,
            commit: self.commit.ok_or_else(|| {
                LockError(format!("lock entry missing `commit` (near line {line})"))
            })?,
            path: self.path,
        })
    }
}

fn strip_comment(line: &str) -> Option<&str> {
    match line.find('#') {
        Some(idx) => {
            let stripped = &line[..idx];
            if stripped.trim().is_empty() {
                None
            } else {
                Some(stripped)
            }
        }
        None => Some(line),
    }
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split_once('=')?;
    Some((k.trim(), v.trim()))
}

fn parse_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockfile_roundtrip() {
        let mut lock = Lockfile::default();
        lock.packages.insert(
            "foo".into(),
            LockPackage {
                git: "https://github.com/org/foo".into(),
                version: "1.2.3".into(),
                commit: "abc123def456".into(),
                path: Some("crates/foo".into()),
            },
        );
        let tmp = std::env::temp_dir().join("coil_lock_roundtrip.lock");
        lock.save(&tmp).unwrap();
        let loaded = Lockfile::load(&tmp).unwrap();
        assert_eq!(loaded, lock);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn parse_ignores_comments_and_blank_lines() {
        let src = r#"
# header
version = 1

[package.foo]
# note
git = "https://example.com/foo"
version = "1.0.0"
commit = "deadbeef"
"#;
        let lock = Lockfile::parse(src).unwrap();
        assert_eq!(lock.packages.len(), 1);
        assert_eq!(lock.packages["foo"].commit, "deadbeef");
    }

    #[test]
    fn parse_rejects_unsupported_lock_version() {
        let err = Lockfile::parse("version = 99\n").unwrap_err();
        assert!(
            err.to_string().contains("unsupported coil.lock version 99"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_rejects_empty_package_name() {
        let err = Lockfile::parse("version = 1\n[package.]\ngit = \"x\"\n").unwrap_err();
        assert!(
            err.to_string().contains("empty package name"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_rejects_unknown_section_and_key() {
        let section = Lockfile::parse("version = 1\n[deps.foo]\n").unwrap_err();
        assert!(
            section.to_string().contains("unknown section"),
            "got: {section}"
        );

        let key = Lockfile::parse(
            "version = 1\n[package.foo]\ngit = \"x\"\nversion = \"1\"\ncommit = \"c\"\nextra = \"no\"\n",
        )
        .unwrap_err();
        assert!(key.to_string().contains("unknown key `extra`"), "got: {key}");
    }

    #[test]
    fn parse_rejects_missing_required_fields() {
        let err = Lockfile::parse(
            "version = 1\n[package.foo]\ngit = \"https://example.com/foo\"\nversion = \"1.0.0\"\n",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("missing `commit`"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_rejects_keys_before_package_section() {
        let err = Lockfile::parse("git = \"https://example.com/foo\"\n").unwrap_err();
        assert!(
            err.to_string().contains("key before `[package.*]`"),
            "got: {err}"
        );
    }
}
