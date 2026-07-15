//! Shared-library path resolution.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::Library;

use super::signature::FfiError;

fn platform_lib_names(stem: &str) -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        vec![format!("{}.dll", stem), format!("lib{}.dll", stem)]
    }
    #[cfg(target_os = "macos")]
    {
        vec![
            stem.to_string(),
            format!("lib{}.dylib", stem),
            format!("{}.dylib", stem),
        ]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        vec![
            stem.to_string(),
            format!("lib{}.so", stem),
            format!("{}.so", stem),
        ]
    }
}

fn push_candidate(out: &mut Vec<PathBuf>, path: PathBuf) {
    if !out.iter().any(|p| p == &path) {
        out.push(path);
    }
}

/// Build an ordered list of paths to try for `name`.
pub fn library_candidates(
    name: &str,
    base_dir: Option<&Path>,
    search_paths: &[PathBuf],
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let path = Path::new(name);

    if path.is_absolute() {
        push_candidate(&mut candidates, path.to_path_buf());
        return candidates;
    }

    if name.contains('/') || name.contains('\\') {
        if let Some(base) = base_dir {
            push_candidate(&mut candidates, base.join(name));
        }
        for root in search_paths {
            push_candidate(&mut candidates, root.join(name));
        }
        push_candidate(&mut candidates, PathBuf::from(name));
        if let Ok(cwd) = std::env::current_dir() {
            push_candidate(&mut candidates, cwd.join(name));
        }
        return candidates;
    }

    // Bare name: try normalized lib names in search dirs, then dlopen search.
    for stem_variant in platform_lib_names(name) {
        if let Some(base) = base_dir {
            push_candidate(&mut candidates, base.join(&stem_variant));
        }
        for root in search_paths {
            push_candidate(&mut candidates, root.join(&stem_variant));
        }
        if let Ok(cwd) = std::env::current_dir() {
            push_candidate(&mut candidates, cwd.join(&stem_variant));
        }
        push_candidate(&mut candidates, PathBuf::from(&stem_variant));
    }

    candidates
}

/// Resolve and load a shared library, trying each candidate in order.
pub fn resolve_library(
    name: &str,
    base_dir: Option<&Path>,
    search_paths: &[PathBuf],
) -> Result<Arc<Library>, FfiError> {
    let candidates = library_candidates(name, base_dir, search_paths);
    let mut errors = Vec::new();

    for candidate in &candidates {
        match unsafe { Library::new(candidate) } {
            Ok(lib) => return Ok(Arc::new(lib)),
            Err(e) => errors.push(format!("{}: {e}", candidate.display())),
        }
    }

    Err(FfiError::LibraryNotFound {
        name: name.to_string(),
        tried: candidates.iter().map(|p| p.display().to_string()).collect(),
        detail: errors.join("; "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_name_generates_lib_prefix_candidates() {
        let base = PathBuf::from("/proj/examples");
        let c = library_candidates("sum", Some(&base), &[]);
        assert!(c.iter().any(|p| p.ends_with("libsum.so") || p.ends_with("libsum.dylib")));
        assert!(c.first().map(|p| p.starts_with(&base)).unwrap_or(false));
    }

    #[test]
    fn relative_path_uses_base_dir_first() {
        let base = PathBuf::from("/proj/examples");
        let c = library_candidates("./libsum.so", Some(&base), &[]);
        assert_eq!(c[0], base.join("./libsum.so"));
    }
}
