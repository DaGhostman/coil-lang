//! Git dependency management: lockfile, install, add, update.
//!
//! Dependencies are declared in `coil.toml` under
//! `[dependencies.<name>]` and materialised under the
//! configured vendor directory (default `vendor/`). There is
//! no central registry — only git remotes (GitHub or otherwise).

mod git;
mod lock;
mod semver_util;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use compiler::{DependencySpec, Manifest};

use self::git::{GitError, auth_url, checkout_commit, commits_between, list_semver_tags, remote_head};
use self::lock::{LockError, LockPackage, Lockfile};
use self::semver_util::{highest_matching, parse_req, parse_version, strip_v_prefix};

pub use self::lock::LOCKFILE_NAME;

/// Errors from dependency commands.
#[derive(Debug)]
pub enum DepsError {
    Io(String),
    Manifest(String),
    Lock(String),
    Git(String),
    Semver(String),
    User(String),
}

impl std::fmt::Display for DepsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DepsError::Io(m)
            | DepsError::Manifest(m)
            | DepsError::Lock(m)
            | DepsError::Git(m)
            | DepsError::Semver(m)
            | DepsError::User(m) => write!(f, "{m}"),
        }
    }
}

impl From<GitError> for DepsError {
    fn from(e: GitError) -> Self {
        DepsError::Git(e.to_string())
    }
}

impl From<LockError> for DepsError {
    fn from(e: LockError) -> Self {
        DepsError::Lock(e.to_string())
    }
}

/// Locate the project root that contains `coil.toml`, walking
/// up from `start` (usually the current working directory).
pub fn find_project_root(start: &Path) -> PathBuf {
    let mut cur = start.to_path_buf();
    loop {
        if cur.join("coil.toml").is_file() {
            return cur;
        }
        if !cur.pop() {
            return start.to_path_buf();
        }
    }
}

/// `coil install` — materialise every dependency from the
/// lockfile (or resolve + lock when the lockfile is missing).
pub fn cmd_install(project_root: &Path) -> Result<(), DepsError> {
    let manifest = load_manifest(project_root)?;
    let lock_path = project_root.join(LOCKFILE_NAME);
    let mut lock = if lock_path.is_file() {
        Lockfile::load(&lock_path)?
    } else {
        Lockfile::default()
    };

    // Drop lock entries that are no longer in the manifest.
    let declared: BTreeSet<_> = manifest.dependencies.keys().cloned().collect();
    lock.packages.retain(|name, _| declared.contains(name));

    for (name, spec) in &manifest.dependencies {
        let entry = if let Some(existing) = lock.packages.get(name).cloned() {
            // Refresh vendor to the locked commit.
            existing
        } else {
            let resolved = resolve_dependency(name, spec)?;
            lock.packages.insert(name.clone(), resolved.clone());
            resolved
        };
        materialise_package(project_root, &manifest, name, &entry)?;
    }

    lock.save(&lock_path)?;
    eprintln!(
        "Installed {} dependenc{} into `{}/`",
        lock.packages.len(),
        if lock.packages.len() == 1 { "y" } else { "ies" },
        manifest.vendor_dir.display()
    );
    Ok(())
}

/// `coil add <name> <git-url> [--version <req>]`
pub fn cmd_add(
    project_root: &Path,
    name: &str,
    git_url: &str,
    version_req: &str,
) -> Result<(), DepsError> {
    validate_package_name(name)?;
    let mut manifest = load_manifest(project_root)?;
    if manifest.dependencies.contains_key(name) {
        return Err(DepsError::User(format!(
            "dependency `{name}` is already declared in coil.toml"
        )));
    }

    let spec = DependencySpec {
        git: git_url.to_string(),
        version: version_req.to_string(),
        path: None,
    };
    let resolved = resolve_dependency(name, &spec)?;
    append_dependency_to_toml(project_root, name, &spec)?;
    manifest.dependencies.insert(name.to_string(), spec);

    let lock_path = project_root.join(LOCKFILE_NAME);
    let mut lock = if lock_path.is_file() {
        Lockfile::load(&lock_path)?
    } else {
        Lockfile::default()
    };
    lock.packages.insert(name.to_string(), resolved.clone());
    materialise_package(project_root, &manifest, name, &resolved)?;
    lock.save(&lock_path)?;

    eprintln!(
        "Added `{name}` {} (commit {}) → {}/{name}/",
        resolved.version,
        &resolved.commit[..resolved.commit.len().min(12)],
        manifest.vendor_dir.display()
    );
    Ok(())
}

/// `coil update [name…]` — bump lock entries to the newest
/// matching semver, show commits since the locked SHA
/// (grouped by repo), and ask before applying.
pub fn cmd_update(project_root: &Path, only: &[String]) -> Result<(), DepsError> {
    let manifest = load_manifest(project_root)?;
    let lock_path = project_root.join(LOCKFILE_NAME);
    if !lock_path.is_file() {
        return Err(DepsError::User(
            "no coil.lock found; run `coil install` first".into(),
        ));
    }
    let mut lock = Lockfile::load(&lock_path)?;

    let targets: Vec<String> = if only.is_empty() {
        manifest.dependencies.keys().cloned().collect()
    } else {
        for name in only {
            if !manifest.dependencies.contains_key(name) {
                return Err(DepsError::User(format!(
                    "`{name}` is not a declared dependency"
                )));
            }
        }
        only.to_vec()
    };

    // Compute proposed updates, grouped by git URL.
    let mut by_repo: BTreeMap<String, Vec<PendingUpdate>> = BTreeMap::new();
    for name in &targets {
        let spec = &manifest.dependencies[name];
        let Some(current) = lock.packages.get(name) else {
            eprintln!("warning: `{name}` is not in coil.lock; run `coil install`");
            continue;
        };
        let proposed = resolve_dependency(name, spec)?;
        if proposed.commit == current.commit {
            eprintln!("`{name}` is already at {} ({})", current.version, short_sha(&current.commit));
            continue;
        }
        let messages = commits_between(&spec.git, &current.commit, &proposed.commit)?;
        by_repo
            .entry(spec.git.clone())
            .or_default()
            .push(PendingUpdate {
                name: name.clone(),
                from_version: current.version.clone(),
                from_commit: current.commit.clone(),
                to: proposed,
                messages,
            });
    }

    if by_repo.is_empty() {
        eprintln!("All selected dependencies are up to date.");
        return Ok(());
    }

    eprintln!("Updates available:\n");
    for (repo, updates) in &by_repo {
        eprintln!("  {repo}");
        for u in updates {
            eprintln!(
                "    {}  {} ({}) → {} ({})",
                u.name,
                u.from_version,
                short_sha(&u.from_commit),
                u.to.version,
                short_sha(&u.to.commit)
            );
            if u.messages.is_empty() {
                eprintln!("      (no commit messages available)");
            } else {
                for msg in &u.messages {
                    eprintln!("      • {msg}");
                }
            }
        }
        eprintln!();
    }

    eprint!("Apply these updates? [y/N] ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|e| DepsError::Io(e.to_string()))?;
    let answer = answer.trim().to_ascii_lowercase();
    if answer != "y" && answer != "yes" {
        eprintln!("Aborted.");
        return Ok(());
    }

    for updates in by_repo.values() {
        for u in updates {
            materialise_package(project_root, &manifest, &u.name, &u.to)?;
            lock.packages.insert(u.name.clone(), u.to.clone());
            eprintln!(
                "Updated `{}` to {} ({})",
                u.name,
                u.to.version,
                short_sha(&u.to.commit)
            );
        }
    }
    lock.save(&lock_path)?;
    Ok(())
}

struct PendingUpdate {
    name: String,
    from_version: String,
    from_commit: String,
    to: LockPackage,
    messages: Vec<String>,
}

fn load_manifest(project_root: &Path) -> Result<Manifest, DepsError> {
    Manifest::load(project_root).map_err(|e| DepsError::Manifest(e.to_string()))
}

fn validate_package_name(name: &str) -> Result<(), DepsError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(DepsError::User(format!(
            "invalid package name `{name}` (use letters, digits, `_`, or `-`)"
        )));
    }
    Ok(())
}

fn resolve_dependency(name: &str, spec: &DependencySpec) -> Result<LockPackage, DepsError> {
    let req = parse_req(&spec.version).map_err(|e| {
        DepsError::Semver(format!("dependency `{name}` has invalid version `{}`: {e}", spec.version))
    })?;

    let tags = list_semver_tags(&spec.git)?;
    let mut candidates: Vec<(semver::Version, String, String)> = Vec::new();
    for (tag, commit) in &tags {
        let ver_str = strip_v_prefix(tag);
        if let Ok(ver) = parse_version(ver_str) {
            if req.matches(&ver) {
                candidates.push((ver, tag.clone(), commit.clone()));
            }
        }
    }

    let (version, _tag, commit) = if let Some(best) = highest_matching(&candidates) {
        best
    } else if spec.version == "*" && tags.is_empty() {
        // No semver tags: lock the default branch tip.
        let head = remote_head(&spec.git)?;
        (
            "0.0.0".to_string(),
            "HEAD".to_string(),
            head,
        )
    } else {
        return Err(DepsError::Semver(format!(
            "no git tag on `{}` matches version requirement `{}` for `{name}`",
            spec.git, spec.version
        )));
    };

    Ok(LockPackage {
        git: spec.git.clone(),
        version,
        commit,
        path: spec.path.clone(),
    })
}

fn materialise_package(
    project_root: &Path,
    manifest: &Manifest,
    name: &str,
    entry: &LockPackage,
) -> Result<(), DepsError> {
    let dest = manifest.package_vendor_path(project_root, name);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| DepsError::Io(e.to_string()))?;
    }
    let url = auth_url(&entry.git);
    checkout_commit(&url, &entry.commit, &dest, entry.path.as_deref())?;
    Ok(())
}

fn append_dependency_to_toml(
    project_root: &Path,
    name: &str,
    spec: &DependencySpec,
) -> Result<(), DepsError> {
    let path = project_root.join("coil.toml");
    let mut contents = if path.is_file() {
        std::fs::read_to_string(&path).map_err(|e| DepsError::Io(e.to_string()))?
    } else {
        String::from("[module]\nroots = [\"./src\"]\n")
    };
    if !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push('\n');
    contents.push_str(&format!("[dependencies.{name}]\n"));
    contents.push_str(&format!("git = \"{}\"\n", spec.git));
    contents.push_str(&format!("version = \"{}\"\n", spec.version));
    if let Some(p) = &spec.path {
        contents.push_str(&format!("path = \"{p}\"\n"));
    }
    std::fs::write(&path, contents).map_err(|e| DepsError::Io(e.to_string()))?;
    Ok(())
}

fn short_sha(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_git_repo(dir: &Path, tags: &[(&str, &str)]) -> String {
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src").join("greet.hy"),
            "fn greet() -> string { return \"hi\"; }\n",
        )
        .unwrap();
        run(dir, &["git", "init", "-b", "main"]);
        run(dir, &["git", "config", "user.email", "test@example.com"]);
        run(dir, &["git", "config", "user.name", "Test"]);
        run(dir, &["git", "add", "."]);
        run(dir, &["git", "commit", "-m", "initial"]);
        for (tag, msg) in tags {
            // Amend content slightly so tags point at distinct commits when needed.
            std::fs::write(
                dir.join("src").join("greet.hy"),
                format!("fn greet() -> string {{ return \"{msg}\"; }}\n"),
            )
            .unwrap();
            run(dir, &["git", "add", "."]);
            run(dir, &["git", "commit", "-m", msg]);
            run(dir, &["git", "tag", tag]);
        }
        dir.display().to_string()
    }

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .status()
            .expect("spawn");
        assert!(status.success(), "failed: {args:?}");
    }

    #[test]
    fn install_add_update_roundtrip_with_local_git_repo() {
        let tmp = std::env::temp_dir().join(format!(
            "coil_deps_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let repo = tmp.join("upstream");
        let url = init_git_repo(&repo, &[("v1.0.0", "release 1.0.0"), ("v1.1.0", "release 1.1.0")]);

        let project = tmp.join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(
            project.join("coil.toml"),
            "[module]\nroots = [\"./src\"]\n",
        )
        .unwrap();

        cmd_add(&project, "foo", &url, "^1.0").unwrap();
        let lock = Lockfile::load(&project.join(LOCKFILE_NAME)).unwrap();
        assert_eq!(lock.packages["foo"].version, "1.1.0");
        assert!(project.join("vendor/foo/src/greet.hy").is_file());

        // Pin lock back to 1.0.0 and bump via update (non-interactive path tested via resolve).
        let tags = list_semver_tags(&url).unwrap();
        let v100 = tags
            .iter()
            .find(|(t, _)| strip_v_prefix(t) == "1.0.0")
            .unwrap()
            .1
            .clone();
        let mut lock = Lockfile::load(&project.join(LOCKFILE_NAME)).unwrap();
        lock.packages.get_mut("foo").unwrap().version = "1.0.0".into();
        lock.packages.get_mut("foo").unwrap().commit = v100.clone();
        lock.save(&project.join(LOCKFILE_NAME)).unwrap();
        materialise_package(
            &project,
            &Manifest::load(&project).unwrap(),
            "foo",
            &lock.packages["foo"].clone(),
        )
        .unwrap();

        // After pinning, resolve again should prefer 1.1.0
        let spec = DependencySpec {
            git: url.clone(),
            version: "^1.0".into(),
            path: None,
        };
        let newest = resolve_dependency("foo", &spec).unwrap();
        assert_eq!(newest.version, "1.1.0");
        let msgs = commits_between(&url, &v100, &newest.commit).unwrap();
        assert!(
            msgs.iter().any(|m| m.contains("release 1.1.0")),
            "expected changelog to include 1.1.0 commit, got {msgs:?}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
