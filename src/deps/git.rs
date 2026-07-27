//! Git helpers for dependency fetch / lock / changelog.

use std::path::Path;
use std::process::Command;

#[derive(Debug)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for GitError {}

/// Rewrite HTTPS GitHub URLs with a token when `GH_TOKEN` or
/// `GITHUB_TOKEN` is set (private repo support). SSH URLs and
/// non-GitHub hosts are returned unchanged — callers rely on
/// the ambient git credential helper / SSH agent for those.
pub fn auth_url(url: &str) -> String {
    let token = std::env::var("GH_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .ok();
    let Some(token) = token else {
        return url.to_string();
    };
    // https://github.com/org/repo.git → https://x-access-token:TOKEN@github.com/org/repo.git
    for prefix in ["https://github.com/", "http://github.com/"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            return format!("https://x-access-token:{token}@github.com/{rest}");
        }
    }
    url.to_string()
}

/// List `(tag_name, commit_sha)` for tags that look like semver
/// on the remote (via `git ls-remote --tags`).
pub fn list_semver_tags(url: &str) -> Result<Vec<(String, String)>, GitError> {
    let url = auth_url(url);
    let output = Command::new("git")
        .args(["ls-remote", "--tags", "--refs", &url])
        .output()
        .map_err(|e| GitError(format!("failed to run git ls-remote: {e}")))?;
    if !output.status.success() {
        return Err(GitError(format!(
            "git ls-remote failed for `{url}`: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut out = Vec::new();
    for line in stdout.lines() {
        // <sha>\trefs/tags/<name>
        let mut parts = line.split_whitespace();
        let Some(sha) = parts.next() else { continue };
        let Some(refname) = parts.next() else { continue };
        let Some(tag) = refname.strip_prefix("refs/tags/") else {
            continue;
        };
        out.push((tag.to_string(), sha.to_string()));
    }
    Ok(out)
}

/// Resolve the default-branch tip commit for `url`.
pub fn remote_head(url: &str) -> Result<String, GitError> {
    let url = auth_url(url);
    let output = Command::new("git")
        .args(["ls-remote", "--symref", &url, "HEAD"])
        .output()
        .map_err(|e| GitError(format!("failed to run git ls-remote HEAD: {e}")))?;
    if !output.status.success() {
        return Err(GitError(format!(
            "git ls-remote HEAD failed for `{url}`: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let Some(sha) = parts.next() else { continue };
        if sha == "ref:" {
            continue;
        }
        if sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(sha.to_string());
        }
    }
    Err(GitError(format!(
        "could not resolve HEAD for `{url}`"
    )))
}

/// Fetch `commit` from `url` into `dest`, optionally copying
/// only the subdirectory `subpath` (monorepo packages).
///
/// Uses a temporary clone then checks out the exact commit so
/// the lockfile SHA is what ends up on disk.
pub fn checkout_commit(
    url: &str,
    commit: &str,
    dest: &Path,
    subpath: Option<&str>,
) -> Result<(), GitError> {
    let url = auth_url(url);
    // Fresh vendor dir each time so we never mix commits.
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| {
            GitError(format!("failed to clear `{}`: {e}", dest.display()))
        })?;
    }
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| GitError(format!("failed to create vendor parent: {e}")))?;

    let staging = parent.join(format!(
        ".coil-staging-{}-{}",
        dest.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("pkg"),
        std::process::id()
    ));
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }

    let status = Command::new("git")
        .args(["clone", "--quiet", "--no-checkout", &url])
        .arg(&staging)
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .map_err(|e| GitError(format!("failed to run git clone: {e}")))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(GitError(format!("git clone failed for `{url}`")));
    }

    // Fetch the exact commit (works for tag tips and arbitrary SHAs).
    let fetch = Command::new("git")
        .args(["fetch", "--depth", "1", "origin", commit])
        .current_dir(&staging)
        .status()
        .map_err(|e| GitError(format!("failed to run git fetch: {e}")))?;
    if !fetch.success() {
        // Fallback: deepen / full fetch for older remotes.
        let fetch2 = Command::new("git")
            .args(["fetch", "origin", commit])
            .current_dir(&staging)
            .status()
            .map_err(|e| GitError(format!("failed to run git fetch: {e}")))?;
        if !fetch2.success() {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(GitError(format!(
                "git fetch of commit `{commit}` failed for `{url}`"
            )));
        }
    }

    let checkout = Command::new("git")
        .args(["-c", "advice.detachedHead=false", "checkout", "--quiet", "--force", commit])
        .current_dir(&staging)
        .status()
        .map_err(|e| GitError(format!("failed to run git checkout: {e}")))?;
    if !checkout.success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(GitError(format!(
            "git checkout of `{commit}` failed"
        )));
    }

    // Drop .git so the vendor tree is a plain source snapshot.
    let _ = std::fs::remove_dir_all(staging.join(".git"));

    let source = match subpath {
        Some(p) if !p.is_empty() => {
            let sub = staging.join(p);
            if !sub.is_dir() {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(GitError(format!(
                    "dependency path `{p}` not found in repository"
                )));
            }
            sub
        }
        _ => staging.clone(),
    };

    std::fs::rename(&source, dest).or_else(|_| {
        // Cross-device rename fallback.
        copy_dir_all(&source, dest)?;
        let _ = std::fs::remove_dir_all(&source);
        Ok(())
    }).map_err(|e: std::io::Error| {
        GitError(format!(
            "failed to place package at `{}`: {e}",
            dest.display()
        ))
    })?;

    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    Ok(())
}

/// Commit subject lines on `url` in `(old_commit, new_commit]`.
pub fn commits_between(
    url: &str,
    old_commit: &str,
    new_commit: &str,
) -> Result<Vec<String>, GitError> {
    if old_commit == new_commit {
        return Ok(Vec::new());
    }
    let url = auth_url(url);
    let tmp = std::env::temp_dir().join(format!(
        "coil-gitlog-{}-{}",
        &new_commit[..new_commit.len().min(8)],
        std::process::id()
    ));
    if tmp.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    let status = Command::new("git")
        .args(["clone", "--no-checkout", &url])
        .arg(&tmp)
        .status()
        .map_err(|e| GitError(format!("failed to run git clone for log: {e}")))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(GitError(format!("git clone failed for changelog on `{url}`")));
    }

    // Fetch both commits so the range is available.
    for commit in [old_commit, new_commit] {
        let _ = Command::new("git")
            .args(["fetch", "--depth", "50", "origin", commit])
            .current_dir(&tmp)
            .status();
    }
    // Ensure we can walk the range; deepen if needed.
    let _ = Command::new("git")
        .args(["fetch", "origin", old_commit, new_commit])
        .current_dir(&tmp)
        .status();

    let range = format!("{old_commit}..{new_commit}");
    let output = Command::new("git")
        .args(["log", "--format=%h %s", &range])
        .current_dir(&tmp)
        .output()
        .map_err(|e| GitError(format!("failed to run git log: {e}")))?;
    let _ = std::fs::remove_dir_all(&tmp);
    if !output.status.success() {
        // Range may be unreachable with shallow history; return empty rather than fail update.
        return Ok(Vec::new());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `auth_url` reads process-wide env vars; serialize those tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn auth_url_rewrites_github_https_when_gh_token_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_gh = std::env::var("GH_TOKEN").ok();
        let old_github = std::env::var("GITHUB_TOKEN").ok();
        // SAFETY: single-threaded under ENV_LOCK for this process's tests.
        unsafe {
            std::env::remove_var("GITHUB_TOKEN");
            std::env::set_var("GH_TOKEN", "secret-token");
        }
        let rewritten = auth_url("https://github.com/org/repo.git");
        assert_eq!(
            rewritten,
            "https://x-access-token:secret-token@github.com/org/repo.git"
        );
        unsafe {
            match old_gh {
                Some(v) => std::env::set_var("GH_TOKEN", v),
                None => std::env::remove_var("GH_TOKEN"),
            }
            match old_github {
                Some(v) => std::env::set_var("GITHUB_TOKEN", v),
                None => std::env::remove_var("GITHUB_TOKEN"),
            }
        }
    }

    #[test]
    fn auth_url_leaves_ssh_and_non_github_urls_unchanged() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_gh = std::env::var("GH_TOKEN").ok();
        let old_github = std::env::var("GITHUB_TOKEN").ok();
        unsafe {
            std::env::set_var("GH_TOKEN", "secret-token");
            std::env::remove_var("GITHUB_TOKEN");
        }
        assert_eq!(
            auth_url("nina.v@example.com:org/repo.git"),
            "nina.v@example.com:org/repo.git"
        );
        assert_eq!(
            auth_url("https://gitlab.com/org/repo.git"),
            "https://gitlab.com/org/repo.git"
        );
        unsafe {
            match old_gh {
                Some(v) => std::env::set_var("GH_TOKEN", v),
                None => std::env::remove_var("GH_TOKEN"),
            }
            match old_github {
                Some(v) => std::env::set_var("GITHUB_TOKEN", v),
                None => std::env::remove_var("GITHUB_TOKEN"),
            }
        }
    }

    #[test]
    fn auth_url_unchanged_without_token() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_gh = std::env::var("GH_TOKEN").ok();
        let old_github = std::env::var("GITHUB_TOKEN").ok();
        unsafe {
            std::env::remove_var("GH_TOKEN");
            std::env::remove_var("GITHUB_TOKEN");
        }
        let url = "https://github.com/org/repo.git";
        assert_eq!(auth_url(url), url);
        unsafe {
            match old_gh {
                Some(v) => std::env::set_var("GH_TOKEN", v),
                None => std::env::remove_var("GH_TOKEN"),
            }
            match old_github {
                Some(v) => std::env::set_var("GITHUB_TOKEN", v),
                None => std::env::remove_var("GITHUB_TOKEN"),
            }
        }
    }
}
