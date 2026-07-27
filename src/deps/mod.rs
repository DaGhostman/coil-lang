//! Git dependency management: lockfile, install, add, update.
//!
//! Dependencies are declared in `coil.toml` under
//! `[dependencies.<name>]` and materialised under the
//! configured vendor directory (default `vendor/`). There is
//! no central registry — only git remotes (GitHub or otherwise).
//!
//! `coil install` walks the **full dependency tree**: each
//! vendored package's own `coil.toml` `[dependencies.*]` are
//! installed too. `coil.lock` is the source of truth for which
//! commit is checked out; when a newer matching tag exists,
//! install prints a notice (use `coil update` to bump).

mod git;
mod lock;
mod semver_util;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use compiler::{DependencySpec, Manifest};

use self::git::{
    GitError, checkout_commit, commits_between, list_semver_tags, remote_head,
};
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

/// `coil install` — materialise the full dependency tree.
///
/// `coil.lock` is the source of truth for commits. Missing lock
/// entries are resolved and appended. After install, prints a
/// notice for any package that has a newer matching tag.
pub fn cmd_install(project_root: &Path) -> Result<(), DepsError> {
    let manifest = load_manifest(project_root)?;
    let lock_path = project_root.join(LOCKFILE_NAME);
    let mut lock = if lock_path.is_file() {
        Lockfile::load(&lock_path)?
    } else {
        Lockfile::default()
    };

    let reqs = sync_dependency_tree(project_root, &manifest, &mut lock)?;
    lock.save(&lock_path)?;

    eprintln!(
        "Installed {} dependenc{} into `{}/`",
        lock.packages.len(),
        if lock.packages.len() == 1 { "y" } else { "ies" },
        manifest.vendor_dir.display()
    );

    emit_newer_version_notices(&lock, &reqs)?;
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
    // Resolve before mutating coil.toml so a bad remote doesn't leave a partial edit.
    let _ = resolve_dependency(name, &spec)?;
    append_dependency_to_toml(project_root, name, &spec)?;
    manifest.dependencies.insert(name.to_string(), spec);

    let lock_path = project_root.join(LOCKFILE_NAME);
    let mut lock = if lock_path.is_file() {
        Lockfile::load(&lock_path)?
    } else {
        Lockfile::default()
    };
    let reqs = sync_dependency_tree(project_root, &manifest, &mut lock)?;
    lock.save(&lock_path)?;

    let resolved = lock.packages.get(name).cloned().ok_or_else(|| {
        DepsError::User(format!("internal error: `{name}` missing from lock after add"))
    })?;

    eprintln!(
        "Added `{name}` {} (commit {}) → {}/{name}/",
        resolved.version,
        short_sha(&resolved.commit),
        manifest.vendor_dir.display()
    );
    emit_newer_version_notices(&lock, &reqs)?;
    Ok(())
}

/// `coil update [name…]` — bump lock entries to the newest
/// matching semver across the dependency tree, show commits
/// since the locked SHA (grouped by repo), and ask before
/// applying (unless `yes`).
pub fn cmd_update(project_root: &Path, only: &[String], yes: bool) -> Result<(), DepsError> {
    let manifest = load_manifest(project_root)?;
    let lock_path = project_root.join(LOCKFILE_NAME);
    if !lock_path.is_file() {
        return Err(DepsError::User(
            "no coil.lock found; run `coil install` first".into(),
        ));
    }
    let mut lock = Lockfile::load(&lock_path)?;

    // Ensure the tree is complete (and collect version reqs) before proposing bumps.
    let reqs = sync_dependency_tree(project_root, &manifest, &mut lock)?;

    let targets: Vec<String> = if only.is_empty() {
        lock.packages.keys().cloned().collect()
    } else {
        for name in only {
            if !lock.packages.contains_key(name) && !manifest.dependencies.contains_key(name) {
                return Err(DepsError::User(format!(
                    "`{name}` is not a declared or locked dependency"
                )));
            }
        }
        only.to_vec()
    };

    let mut by_repo: BTreeMap<String, Vec<PendingUpdate>> = BTreeMap::new();
    for name in &targets {
        let Some(current) = lock.packages.get(name) else {
            eprintln!("warning: `{name}` is not in coil.lock; run `coil install`");
            continue;
        };
        let Some(req_list) = reqs.get(name) else {
            continue;
        };
        let proposed = resolve_with_reqs(name, &current.git, current.path.as_deref(), req_list)?;
        if proposed.commit == current.commit {
            eprintln!(
                "`{name}` is already at {} ({})",
                current.version,
                short_sha(&current.commit)
            );
            continue;
        }
        let messages = commits_between(&current.git, &current.commit, &proposed.commit)?;
        by_repo
            .entry(current.git.clone())
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
        lock.save(&lock_path)?;
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

    if !yes {
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

    // Re-walk so newly unlocked transitive deps of bumped packages are locked too.
    let _ = sync_dependency_tree(project_root, &manifest, &mut lock)?;
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

/// Walk root + transitive `[dependencies.*]`, using the lock as
/// commit SoT when present. Returns the merged version-requirement
/// lists per package name (for newer-version notices / update).
fn sync_dependency_tree(
    project_root: &Path,
    root_manifest: &Manifest,
    lock: &mut Lockfile,
) -> Result<BTreeMap<String, Vec<String>>, DepsError> {
    let mut queue: VecDeque<(String, DependencySpec)> = VecDeque::new();
    for (name, spec) in &root_manifest.dependencies {
        queue.push_back((name.clone(), spec.clone()));
    }

    let mut reqs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut specs: BTreeMap<String, DependencySpec> = BTreeMap::new();
    let mut finished: BTreeSet<String> = BTreeSet::new();

    while let Some((name, spec)) = queue.pop_front() {
        validate_package_name(&name)?;

        if let Some(prev) = specs.get(&name) {
            if prev.git != spec.git || prev.path != spec.path {
                return Err(DepsError::User(format!(
                    "dependency `{name}` resolved from conflicting sources:\n  - {}\n  - {}",
                    format_spec(prev),
                    format_spec(&spec)
                )));
            }
            reqs.entry(name.clone())
                .or_default()
                .push(spec.version.clone());
            // Already queued/processed with same source; still need locked
            // version to satisfy this additional req (checked below if finished).
            if finished.contains(&name) {
                let locked = lock.packages.get(&name).ok_or_else(|| {
                    DepsError::User(format!("internal: `{name}` finished but not locked"))
                })?;
                ensure_version_satisfies(&name, &locked.version, &spec.version)?;
            }
            continue;
        }

        specs.insert(name.clone(), spec.clone());
        reqs.entry(name.clone())
            .or_default()
            .push(spec.version.clone());

        let entry = if let Some(existing) = lock.packages.get(&name).cloned() {
            if existing.git != spec.git || existing.path != spec.path {
                return Err(DepsError::User(format!(
                    "coil.lock entry `{name}` does not match coil.toml \
                     (lock: {}; manifest: {}). Delete the lock entry or run `coil update`.",
                    format_lock(&existing),
                    format_spec(&spec)
                )));
            }
            ensure_version_satisfies(&name, &existing.version, &spec.version)?;
            existing
        } else {
            let resolved = resolve_with_reqs(
                &name,
                &spec.git,
                spec.path.as_deref(),
                std::slice::from_ref(&spec.version),
            )?;
            lock.packages.insert(name.clone(), resolved.clone());
            resolved
        };

        materialise_package(project_root, root_manifest, &name, &entry)?;

        // Transitive: read the package's own coil.toml.
        let pkg_dir = root_manifest.package_vendor_path(project_root, &name);
        let pkg_manifest = Manifest::load(&pkg_dir).map_err(|e| {
            DepsError::Manifest(format!(
                "failed to read package `{name}` manifest at `{}`: {e}",
                pkg_dir.display()
            ))
        })?;
        for (dep_name, dep_spec) in pkg_manifest.dependencies {
            queue.push_back((dep_name, dep_spec));
        }

        finished.insert(name);
    }

    // Drop lock entries that are no longer reachable from the root tree.
    lock.packages.retain(|name, _| finished.contains(name));
    Ok(reqs)
}

fn emit_newer_version_notices(
    lock: &Lockfile,
    reqs: &BTreeMap<String, Vec<String>>,
) -> Result<(), DepsError> {
    // Skip remote version probes in CI / offline installs (no network churn).
    if std::env::var_os("CI").is_some() || std::env::var_os("COIL_OFFLINE").is_some() {
        return Ok(());
    }
    for (name, locked) in &lock.packages {
        let Some(req_list) = reqs.get(name) else {
            continue;
        };
        match resolve_with_reqs(name, &locked.git, locked.path.as_deref(), req_list) {
            Ok(newest) if newest.commit != locked.commit => {
                eprintln!(
                    "notice: `{name}` has a newer version {} available (locked at {}); run `coil update` to upgrade",
                    newest.version, locked.version
                );
            }
            Ok(_) => {}
            Err(e) => {
                // Non-fatal: network blips shouldn't fail install after materialise.
                eprintln!("warning: could not check for newer `{name}`: {e}");
            }
        }
    }
    Ok(())
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

fn ensure_version_satisfies(name: &str, locked_ver: &str, req_str: &str) -> Result<(), DepsError> {
    let req = parse_req(req_str).map_err(|e| {
        DepsError::Semver(format!(
            "dependency `{name}` has invalid version `{req_str}`: {e}"
        ))
    })?;
    let ver = parse_version(strip_v_prefix(locked_ver)).map_err(|e| {
        DepsError::Semver(format!(
            "locked version `{locked_ver}` for `{name}` is not semver: {e}"
        ))
    })?;
    if !req.matches(&ver) {
        return Err(DepsError::User(format!(
            "locked `{name}` {locked_ver} does not satisfy requirement `{req_str}`; run `coil update`"
        )));
    }
    Ok(())
}

fn resolve_dependency(name: &str, spec: &DependencySpec) -> Result<LockPackage, DepsError> {
    resolve_with_reqs(name, &spec.git, spec.path.as_deref(), std::slice::from_ref(&spec.version))
}

fn resolve_with_reqs(
    name: &str,
    git: &str,
    path: Option<&str>,
    req_strs: &[String],
) -> Result<LockPackage, DepsError> {
    let mut reqs = Vec::with_capacity(req_strs.len());
    for r in req_strs {
        reqs.push(parse_req(r).map_err(|e| {
            DepsError::Semver(format!(
                "dependency `{name}` has invalid version `{r}`: {e}"
            ))
        })?);
    }

    let tags = list_semver_tags(git)?;
    let mut candidates: Vec<(semver::Version, String, String)> = Vec::new();
    for (tag, commit) in &tags {
        let ver_str = strip_v_prefix(tag);
        if let Ok(ver) = parse_version(ver_str) {
            if reqs.iter().all(|r| r.matches(&ver)) {
                candidates.push((ver, tag.clone(), commit.clone()));
            }
        }
    }

    let (version, _tag, commit) = if let Some(best) = highest_matching(&candidates) {
        best
    } else if req_strs.iter().all(|r| r == "*") && tags.is_empty() {
        let head = remote_head(git)?;
        ("0.0.0".to_string(), "HEAD".to_string(), head)
    } else {
        return Err(DepsError::Semver(format!(
            "no git tag on `{}` matches version requirement(s) [{}] for `{name}`",
            git,
            req_strs.join(", ")
        )));
    };

    Ok(LockPackage {
        git: git.to_string(),
        version,
        commit,
        path: path.map(|s| s.to_string()),
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
    checkout_commit(&entry.git, &entry.commit, &dest, entry.path.as_deref())?;
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
    contents.push_str(&format!("git = \"{}\"\n", escape_toml_string(&spec.git)?));
    contents.push_str(&format!(
        "version = \"{}\"\n",
        escape_toml_string(&spec.version)?
    ));
    if let Some(p) = &spec.path {
        contents.push_str(&format!("path = \"{}\"\n", escape_toml_string(p)?));
    }
    std::fs::write(&path, contents).map_err(|e| DepsError::Io(e.to_string()))?;
    Ok(())
}

fn escape_toml_string(s: &str) -> Result<String, DepsError> {
    lock::escape_toml_string(s).map_err(|e| DepsError::User(e.0.replace("coil.lock", "coil.toml")))
}

fn format_spec(spec: &DependencySpec) -> String {
    match &spec.path {
        Some(p) => format!("{} (path={p}, version={})", spec.git, spec.version),
        None => format!("{} (version={})", spec.git, spec.version),
    }
}

fn format_lock(pkg: &LockPackage) -> String {
    match &pkg.path {
        Some(p) => format!("{} (path={p}, version={})", pkg.git, pkg.version),
        None => format!("{} (version={})", pkg.git, pkg.version),
    }
}

fn short_sha(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_workdir(label: &str) -> PathBuf {
        let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "coil_deps_{label}_{}_{n}",
            std::process::id()
        ))
    }

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

    /// Repo whose coil.toml depends on `child` at `child_url`.
    fn init_parent_repo(dir: &Path, child_url: &str, tag: &str) -> String {
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src").join("parent.hy"),
            "fn parent() -> string { return \"parent\"; }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("coil.toml"),
            format!(
                "[module]\nroots = [\"./src\"]\n\n[dependencies.child]\ngit = \"{child_url}\"\nversion = \"^1.0\"\n"
            ),
        )
        .unwrap();
        run(dir, &["git", "init", "-b", "main"]);
        run(dir, &["git", "config", "user.email", "test@example.com"]);
        run(dir, &["git", "config", "user.name", "Test"]);
        run(dir, &["git", "add", "."]);
        run(dir, &["git", "commit", "-m", "parent initial"]);
        run(dir, &["git", "tag", tag]);
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
        let tmp = temp_workdir("test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let repo = tmp.join("upstream");
        let url = init_git_repo(
            &repo,
            &[("v1.0.0", "release 1.0.0"), ("v1.1.0", "release 1.1.0")],
        );

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

    #[test]
    fn install_pulls_transitive_dependencies() {
        let tmp = temp_workdir("transitive");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let child = tmp.join("child");
        let child_url = init_git_repo(&child, &[("v1.0.0", "child 1.0.0")]);

        let parent = tmp.join("parent");
        let parent_url = init_parent_repo(&parent, &child_url, "v1.0.0");

        let project = tmp.join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(
            project.join("coil.toml"),
            format!(
                "[module]\nroots = [\"./src\"]\n\n[dependencies.parent]\ngit = \"{parent_url}\"\nversion = \"^1.0\"\n"
            ),
        )
        .unwrap();

        cmd_install(&project).unwrap();
        let lock = Lockfile::load(&project.join(LOCKFILE_NAME)).unwrap();
        assert!(
            lock.packages.contains_key("parent"),
            "parent missing from lock"
        );
        assert!(
            lock.packages.contains_key("child"),
            "transitive child missing from lock: {:?}",
            lock.packages.keys().collect::<Vec<_>>()
        );
        assert!(project.join("vendor/parent/src/parent.hy").is_file());
        assert!(project.join("vendor/child/src/greet.hy").is_file());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn lockfile_is_source_of_truth_on_reinstall() {
        let tmp = temp_workdir("sot");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let repo = tmp.join("upstream");
        let url = init_git_repo(
            &repo,
            &[("v1.0.0", "release 1.0.0"), ("v1.1.0", "release 1.1.0")],
        );

        let project = tmp.join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(
            project.join("coil.toml"),
            format!(
                "[module]\nroots = [\"./src\"]\n\n[dependencies.foo]\ngit = \"{url}\"\nversion = \"^1.0\"\n"
            ),
        )
        .unwrap();

        cmd_install(&project).unwrap();
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

        // Re-install must keep the locked 1.0.0 commit (SoT), not jump to 1.1.0.
        cmd_install(&project).unwrap();
        let lock = Lockfile::load(&project.join(LOCKFILE_NAME)).unwrap();
        assert_eq!(lock.packages["foo"].version, "1.0.0");
        assert_eq!(lock.packages["foo"].commit, v100);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn add_rejects_invalid_and_duplicate_package_names() {
        let tmp = temp_workdir("name_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(
            tmp.join("coil.toml"),
            "[module]\nroots = [\"./src\"]\n",
        )
        .unwrap();

        let repo = tmp.join("upstream");
        let url = init_git_repo(&repo, &[("v1.0.0", "one")]);

        let bad = cmd_add(&tmp, "bad/name", &url, "*").unwrap_err();
        assert!(
            bad.to_string().contains("invalid package name"),
            "got: {bad}"
        );

        cmd_add(&tmp, "foo", &url, "*").unwrap();
        let dup = cmd_add(&tmp, "foo", &url, "*").unwrap_err();
        assert!(
            dup.to_string().contains("already declared"),
            "got: {dup}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn install_drops_stale_lock_entries_not_in_manifest() {
        let tmp = temp_workdir("stale_lock");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();

        let repo = tmp.join("upstream");
        let url = init_git_repo(&repo, &[("v1.0.0", "one")]);
        std::fs::write(
            tmp.join("coil.toml"),
            format!(
                "[module]\nroots = [\"./src\"]\n\n[dependencies.foo]\ngit = \"{url}\"\nversion = \"^1.0\"\n"
            ),
        )
        .unwrap();

        // Seed a lockfile with an extra package that is no longer declared.
        let mut lock = Lockfile::default();
        lock.packages.insert(
            "gone".into(),
            LockPackage {
                git: url.clone(),
                version: "1.0.0".into(),
                commit: "0".repeat(40),
                path: None,
            },
        );
        lock.save(&tmp.join(LOCKFILE_NAME)).unwrap();

        cmd_install(&tmp).unwrap();
        let lock = Lockfile::load(&tmp.join(LOCKFILE_NAME)).unwrap();
        assert!(lock.packages.contains_key("foo"));
        assert!(
            !lock.packages.contains_key("gone"),
            "stale lock entry should be pruned: {:?}",
            lock.packages.keys().collect::<Vec<_>>()
        );
        assert!(tmp.join("vendor/foo/src/greet.hy").is_file());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_star_with_no_tags_locks_default_branch_head() {
        let tmp = temp_workdir("head_fallback");
        let _ = std::fs::remove_dir_all(&tmp);
        let repo = tmp.join("upstream");
        // No tags — version `*` should fall back to HEAD.
        let url = init_git_repo(&repo, &[]);
        let head = remote_head(&url).unwrap();

        let resolved = resolve_dependency(
            "untagged",
            &DependencySpec {
                git: url,
                version: "*".into(),
                path: None,
            },
        )
        .unwrap();
        assert_eq!(resolved.version, "0.0.0");
        assert_eq!(resolved.commit, head);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn materialise_honors_monorepo_subpath() {
        let tmp = temp_workdir("monorepo");
        let _ = std::fs::remove_dir_all(&tmp);
        let repo = tmp.join("upstream");
        std::fs::create_dir_all(repo.join("packages/lib/src")).unwrap();
        std::fs::write(
            repo.join("packages/lib/src/mod.hy"),
            "fn hi() -> string { return \"mono\"; }\n",
        )
        .unwrap();
        run(&repo, &["git", "init", "-b", "main"]);
        run(&repo, &["git", "config", "user.email", "test@example.com"]);
        run(&repo, &["git", "config", "user.name", "Test"]);
        run(&repo, &["git", "add", "."]);
        run(&repo, &["git", "commit", "-m", "initial"]);
        run(&repo, &["git", "tag", "v0.1.0"]);
        let url = repo.display().to_string();

        let project = tmp.join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(
            project.join("coil.toml"),
            "[module]\nroots = [\"./src\"]\n",
        )
        .unwrap();

        let spec = DependencySpec {
            git: url.clone(),
            version: "^0.1".into(),
            path: Some("packages/lib".into()),
        };
        let resolved = resolve_dependency("lib", &spec).unwrap();
        let manifest = Manifest {
            roots: vec![PathBuf::from("./src")],
            entry: None,
            ffi_search_paths: Vec::new(),
            vendor_dir: PathBuf::from("vendor"),
            dependencies: BTreeMap::from([("lib".into(), spec)]),
        };
        materialise_package(&project, &manifest, "lib", &resolved).unwrap();
        assert!(
            project.join("vendor/lib/src/mod.hy").is_file(),
            "expected monorepo subpath contents under vendor/lib/"
        );
        assert!(
            !project.join("vendor/lib/packages").exists(),
            "vendor tree should be the subpath root, not the repo root"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn update_errors_without_lockfile_or_undeclared_name() {
        let tmp = temp_workdir("update_err");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(
            tmp.join("coil.toml"),
            "[module]\nroots = [\"./src\"]\n",
        )
        .unwrap();

        let missing_lock = cmd_update(&tmp, &[], true).unwrap_err();
        assert!(
            missing_lock.to_string().contains("no coil.lock"),
            "got: {missing_lock}"
        );

        // Create an empty lock so the undeclared-name path is reachable.
        Lockfile::default()
            .save(&tmp.join(LOCKFILE_NAME))
            .unwrap();
        let bad_name = cmd_update(&tmp, &["not_declared".into()], true).unwrap_err();
        assert!(
            bad_name
                .to_string()
                .contains("not a declared or locked dependency"),
            "got: {bad_name}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_project_root_walks_up_to_coil_toml() {
        let tmp = temp_workdir("root");
        let _ = std::fs::remove_dir_all(&tmp);
        let nested = tmp.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(tmp.join("coil.toml"), "[module]\nroots = [\"./src\"]\n").unwrap();
        assert_eq!(find_project_root(&nested), tmp);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
