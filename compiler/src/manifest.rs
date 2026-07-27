//! Project manifest (`coil.toml`) parsing and module path resolution.
//!
//! A `coil.toml` at the project root declares search roots for `use`
//! resolution, optional git dependencies, and an optional entry point.
//! The pipeline maps `a::b::c` paths to `<root>/a/b/c.hy` files on disk.
//! Declared packages are also reachable as `name::…` under the vendor
//! directory (see [`DependencySpec`]).
//!
//! ## Format
//!
//! ```toml
//! [package]
//! vendor_dir = "vendor"
//!
//! [module]
//! roots = ["./src", "./builtins"]
//!
//! [entry]
//! # Optional. Default = the file passed to the compiler.
//! file = "./src/main.hy"
//!
//! [dependencies.foo]
//! git = "https://github.com/org/foo"
//! version = "^1.0.0"
//! ```
//!
//! The parser is intentionally minimal (no nested tables beyond
//! dotted section names, no inline tables, no arrays of tables).
//! The grammar is:
//!
//! ```text
//! file   := section* ; zero or more sections
//! section := '[' section_name ']' '\n' (entry '\n')*
//! section_name := ident | 'dependencies.' ident
//! entry  := key '=' value '\n'
//! key    := ident (no quotes, no spaces)
//! value  := string | array
//! string := '"' char* '"'
//! array  := '[' (string (',' string)*)? ']'
//! ```
//!
//! Comments start with `#` and run to end of line. Whitespace
//! at line boundaries is ignored.
//!
//! ## Discovery algorithm
//!
//! Given `use a::b::c;` and a manifest with roots
//! `["./src", "./vendor"]`, we search each root in order
//! and return the first file that exists:
//!
//! 1. `./src/a/b/c.hy`
//! 2. `./vendor/a/b/c.hy`
//!
//! The first match wins. If no project root contains the file,
//! and the first path segment names a declared dependency, we
//! resolve the remainder inside that package's vendor checkout
//! (namespace prefix = the package name).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Errors that can occur while loading a `coil.toml` manifest.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // some variants are reserved for future strict-mode validation
pub enum ManifestError {
    /// The manifest file could not be read (I/O error).
    Io(String),
    /// A line failed to parse (invalid syntax).
    Parse { line: usize, message: String },
    /// A required section is missing.
    MissingSection(&'static str),
    /// A required key is missing from a section.
    #[allow(dead_code)] // reserved for future strict-mode validation
    MissingKey {
        section: &'static str,
        key: &'static str,
    },
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Io(msg) => write!(f, "manifest I/O error: {}", msg),
            ManifestError::Parse { line, message } => {
                write!(f, "manifest parse error at line {}: {}", line, message)
            }
            ManifestError::MissingSection(s) => write!(f, "missing manifest section: [{}]", s),
            ManifestError::MissingKey { section, key } => {
                write!(f, "missing manifest key: {}.{}", section, key)
            }
        }
    }
}

impl std::error::Error for ManifestError {}

/// A git dependency declared under `[dependencies.<name>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencySpec {
    /// Git remote URL (HTTPS or SSH). Private repos use the
    /// caller's git credentials / `GH_TOKEN` / `GITHUB_TOKEN`.
    pub git: String,
    /// Semver version requirement string (e.g. `"^1.2.0"`).
    pub version: String,
    /// Optional subdirectory inside the cloned repo (monorepos).
    pub path: Option<String>,
}

/// Resolved project manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// Search roots for module discovery. Each path is
    /// resolved relative to the project root (the directory
    /// containing `coil.toml`).
    pub roots: Vec<PathBuf>,
    /// Optional explicit entry point. When `None`, the
    /// compiler falls back to the file passed on the CLI.
    pub entry: Option<PathBuf>,
    /// Extra directories searched when resolving FFI library paths.
    pub ffi_search_paths: Vec<PathBuf>,
    /// Directory under the project root where `coil install`
    /// materializes git dependencies. Defaults to `"vendor"`.
    pub vendor_dir: PathBuf,
    /// Declared git dependencies keyed by package name. The
    /// name is the `use` namespace prefix (`foo` → `foo::…`).
    pub dependencies: BTreeMap<String, DependencySpec>,
}

impl Default for Manifest {
    /// Default manifest when no `coil.toml` is present:
    /// a single search root at `src/`, no explicit entry
    /// point, vendor dir `vendor`, no dependencies.
    fn default() -> Self {
        Self {
            roots: vec![PathBuf::from("src")],
            entry: None,
            ffi_search_paths: Vec::new(),
            vendor_dir: PathBuf::from("vendor"),
            dependencies: BTreeMap::new(),
        }
    }
}

impl Manifest {
    /// Load a manifest from a project root. If `coil.toml`
    /// exists, parse it. If not, return the default manifest
    /// (just `src/`).
    ///
    /// `project_root` is the directory containing the
    /// `coil.toml` file. Search roots in the manifest are
    /// stored as relative paths; callers should re-root them
    /// when actually searching (see
    /// [`Manifest::resolve_module`]).
    pub fn load(project_root: &Path) -> Result<Self, ManifestError> {
        let manifest_path = project_root.join("coil.toml");
        match std::fs::read_to_string(&manifest_path) {
            Ok(contents) => Self::parse(&contents),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(ManifestError::Io(format!(
                "failed to read `{}`: {e}",
                manifest_path.display()
            ))),
        }
    }

    /// Byte range covering the content of `line_num` (1-based), for diagnostics.
    pub fn byte_range_for_line(source: &str, line_num: usize) -> std::ops::Range<usize> {
        if line_num == 0 {
            return 0..0;
        }
        let mut offset = 0usize;
        for (idx, line) in source.lines().enumerate() {
            if idx + 1 == line_num {
                return offset..offset + line.len();
            }
            offset += line.len();
            if offset < source.len() && source.as_bytes()[offset] == b'\n' {
                offset += 1;
            }
        }
        0..0
    }

    /// Parse a manifest from its source text. Exposed for
    /// tests; production code uses [`Self::load`].
    pub fn parse(source: &str) -> Result<Self, ManifestError> {
        let mut roots: Option<Vec<PathBuf>> = None;
        let mut entry: Option<PathBuf> = None;
        let mut ffi_search_paths: Option<Vec<PathBuf>> = None;
        let mut vendor_dir: Option<PathBuf> = None;
        let mut dependencies: BTreeMap<String, DependencySpec> = BTreeMap::new();
        // Pending dependency fields while inside `[dependencies.name]`.
        let mut pending_dep: Option<(String, PendingDep)> = None;
        let mut current_section: Option<SectionKind> = None;

        for (idx, raw_line) in source.lines().enumerate() {
            let line_num = idx + 1;
            // Strip comment and surrounding whitespace.
            let line = match strip_comment(raw_line) {
                Some(l) => l.trim(),
                None => continue, // line was entirely a comment
            };
            if line.is_empty() {
                continue;
            }

            // Section header: `[name]` or `[dependencies.name]`.
            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                // Flush any open dependency section.
                if let Some((dep_name, pending)) = pending_dep.take() {
                    dependencies.insert(dep_name, pending.into_spec(line_num)?);
                }
                let name = name.trim();
                current_section = Some(match name {
                    "module" => SectionKind::Module,
                    "entry" => SectionKind::Entry,
                    "ffi" => SectionKind::Ffi,
                    "package" => SectionKind::Package,
                    other if other.starts_with("dependencies.") => {
                        let dep_name = other["dependencies.".len()..].trim();
                        if dep_name.is_empty()
                            || !dep_name
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                        {
                            return Err(ManifestError::Parse {
                                line: line_num,
                                message: format!(
                                    "invalid dependency section `[{}]` (expected `[dependencies.<name>]`)",
                                    other
                                ),
                            });
                        }
                        if dependencies.contains_key(dep_name) {
                            return Err(ManifestError::Parse {
                                line: line_num,
                                message: format!("duplicate dependency `{}`", dep_name),
                            });
                        }
                        pending_dep = Some((dep_name.to_string(), PendingDep::default()));
                        SectionKind::Dependency
                    }
                    other => {
                        return Err(ManifestError::Parse {
                            line: line_num,
                            message: format!("unknown section `[{}]`", other),
                        });
                    }
                });
                continue;
            }

            // Key-value entry: `key = value`.
            let section = current_section.ok_or(ManifestError::Parse {
                line: line_num,
                message: "key-value entry before any section header".to_string(),
            })?;

            let (key, value) = parse_kv(line).ok_or(ManifestError::Parse {
                line: line_num,
                message: format!("expected `key = value`, got `{}`", line),
            })?;

            match section {
                SectionKind::Module if key == "roots" => {
                    let parsed = parse_string_array(value).ok_or(ManifestError::Parse {
                        line: line_num,
                        message: format!("expected array of strings, got `{}`", value),
                    })?;
                    roots = Some(parsed.into_iter().map(PathBuf::from).collect());
                }
                SectionKind::Entry if key == "file" => {
                    let parsed = parse_string(value).ok_or(ManifestError::Parse {
                        line: line_num,
                        message: format!("expected string, got `{}`", value),
                    })?;
                    entry = Some(PathBuf::from(parsed));
                }
                SectionKind::Ffi if key == "search_paths" => {
                    let parsed = parse_string_array(value).ok_or(ManifestError::Parse {
                        line: line_num,
                        message: format!("expected array of strings, got `{}`", value),
                    })?;
                    ffi_search_paths = Some(parsed.into_iter().map(PathBuf::from).collect());
                }
                SectionKind::Package if key == "vendor_dir" => {
                    let parsed = parse_string(value).ok_or(ManifestError::Parse {
                        line: line_num,
                        message: format!("expected string, got `{}`", value),
                    })?;
                    vendor_dir = Some(PathBuf::from(parsed));
                }
                SectionKind::Dependency => {
                    let (_, pending) = pending_dep.as_mut().expect("dependency section open");
                    let parsed = parse_string(value).ok_or(ManifestError::Parse {
                        line: line_num,
                        message: format!("expected string, got `{}`", value),
                    })?;
                    match key {
                        "git" => pending.git = Some(parsed),
                        "version" => pending.version = Some(parsed),
                        "path" => pending.path = Some(parsed),
                        other => {
                            return Err(ManifestError::Parse {
                                line: line_num,
                                message: format!("unknown key `dependencies.*.{}`", other),
                            });
                        }
                    }
                }
                other => {
                    let section_name = match other {
                        SectionKind::Module => "module",
                        SectionKind::Entry => "entry",
                        SectionKind::Ffi => "ffi",
                        SectionKind::Package => "package",
                        SectionKind::Dependency => "dependencies",
                    };
                    return Err(ManifestError::Parse {
                        line: line_num,
                        message: format!("unknown key `{}.{}`", section_name, key),
                    });
                }
            }
        }

        if let Some((dep_name, pending)) = pending_dep.take() {
            // End-of-file flush: use a synthetic line number.
            let line = source.lines().count().max(1);
            dependencies.insert(dep_name, pending.into_spec(line)?);
        }

        Ok(Self {
            roots: roots.unwrap_or_else(|| vec![PathBuf::from("src")]),
            entry,
            ffi_search_paths: ffi_search_paths.unwrap_or_default(),
            vendor_dir: vendor_dir.unwrap_or_else(|| PathBuf::from("vendor")),
            dependencies,
        })
    }

    /// Absolute path to the vendored checkout for `name`
    /// (`<project>/<vendor_dir>/<name>`).
    pub fn package_vendor_path(&self, project_root: &Path, name: &str) -> PathBuf {
        project_root.join(&self.vendor_dir).join(name)
    }

    /// Resolve a `use` target (`a::b::c`) to an absolute file
    /// path. Searches each search root in order; the first
    /// match wins. If no project root matches and the first
    /// path segment is a declared dependency name, resolves
    /// inside that package's vendor directory.
    ///
    /// `path` is the segments of the module path BEFORE the
    /// item name (e.g. `["a", "b"]` for `use a::b::c;`).
    /// `name` is the final segment (e.g. `"c"`).
    pub fn resolve_use(&self, project_root: &Path, path: &[String], name: &str) -> Option<PathBuf> {
        for root in &self.roots {
            let mut candidate = project_root.join(root);
            for segment in path {
                candidate.push(segment);
            }
            candidate.push(format!("{}.hy", name));
            if candidate.exists() {
                return Some(candidate);
            }
        }
        self.resolve_use_in_package(project_root, path, name)
    }

    /// Resolve a `mod foo;` forward declaration to an
    /// absolute file path. Looks for `<root>/foo.hy` in
    /// each search root.
    pub fn resolve_mod(&self, project_root: &Path, name: &str) -> Option<PathBuf> {
        for root in &self.roots {
            let candidate = project_root.join(root).join(format!("{}.hy", name));
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }

    /// Compute the namespace of a file given its absolute
    /// path and the project root. Prefers project search
    /// roots; if the file lives under a vendored package,
    /// returns `package_name::relative`.
    pub fn namespace_of(&self, project_root: &Path, file: &Path) -> Option<String> {
        for root in &self.roots {
            let abs_root = project_root.join(root);
            if let Ok(rel) = file.strip_prefix(&abs_root) {
                return Some(path_to_namespace(rel));
            }
        }
        for (pkg_name, _spec) in &self.dependencies {
            let pkg_dir = self.package_vendor_path(project_root, pkg_name);
            let pkg_roots = package_module_roots(&pkg_dir);
            for pkg_root in &pkg_roots {
                let abs = pkg_dir.join(pkg_root);
                if let Ok(rel) = file.strip_prefix(&abs) {
                    let inner = path_to_namespace(rel);
                    if inner.is_empty() {
                        return Some(pkg_name.clone());
                    }
                    return Some(format!("{}::{}", pkg_name, inner));
                }
            }
            // Fallback: relative to the package checkout root.
            if let Ok(rel) = file.strip_prefix(&pkg_dir) {
                let inner = path_to_namespace(rel);
                if inner.is_empty() {
                    return Some(pkg_name.clone());
                }
                return Some(format!("{}::{}", pkg_name, inner));
            }
        }
        None
    }

    fn resolve_use_in_package(
        &self,
        project_root: &Path,
        path: &[String],
        name: &str,
    ) -> Option<PathBuf> {
        let (pkg_name, rest): (&str, &[String]) = if path.is_empty() {
            // `use foo;` where foo is both the package and the module stem —
            // only valid via resolve_mod; keep None here.
            return None;
        } else {
            (path[0].as_str(), &path[1..])
        };
        if !self.dependencies.contains_key(pkg_name) {
            return None;
        }
        let pkg_dir = self.package_vendor_path(project_root, pkg_name);
        if !pkg_dir.is_dir() {
            return None;
        }
        let pkg_roots = package_module_roots(&pkg_dir);
        for pkg_root in &pkg_roots {
            let mut candidate = pkg_dir.join(pkg_root);
            for segment in rest {
                candidate.push(segment);
            }
            candidate.push(format!("{}.hy", name));
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionKind {
    Module,
    Entry,
    Ffi,
    Package,
    Dependency,
}

#[derive(Debug, Default)]
struct PendingDep {
    git: Option<String>,
    version: Option<String>,
    path: Option<String>,
}

impl PendingDep {
    fn into_spec(self, line: usize) -> Result<DependencySpec, ManifestError> {
        let git = self.git.ok_or_else(|| ManifestError::Parse {
            line,
            message: "dependency missing required `git` key".to_string(),
        })?;
        let version = self.version.unwrap_or_else(|| "*".to_string());
        Ok(DependencySpec {
            git,
            version,
            path: self.path,
        })
    }
}

/// Module search roots inside a vendored package directory.
/// Reads the package's own `coil.toml` `[module].roots` when
/// present; otherwise defaults to `["src", "."]`.
pub fn package_module_roots(pkg_dir: &Path) -> Vec<PathBuf> {
    let manifest_path = pkg_dir.join("coil.toml");
    if let Ok(contents) = std::fs::read_to_string(&manifest_path) {
        if let Ok(m) = Manifest::parse(&contents) {
            if !m.roots.is_empty() {
                return m.roots;
            }
        }
    }
    vec![PathBuf::from("src"), PathBuf::from(".")]
}

/// Strip an inline comment (everything after `#`, but not
/// inside a string). Returns `None` if the line is entirely a
/// comment (or empty after stripping).
fn strip_comment(line: &str) -> Option<&str> {
    // We don't track string boundaries here because our
    // manifest format doesn't allow `#` inside strings in
    // practice (paths and section names don't include `#`).
    // If we ever allow richer values, this becomes more
    // involved.
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

/// Parse a `key = value` line. Returns `(key, value)` where
/// `value` is the un-parsed RHS (caller decides whether it's
/// a string, array, etc.).
fn parse_kv(line: &str) -> Option<(&str, &str)> {
    let (key, rest) = line.split_once('=')?;
    Some((key.trim(), rest.trim()))
}

/// Parse a TOML-like double-quoted string. Returns the inner
/// text (without the surrounding quotes). Returns `None` if
/// the value isn't a double-quoted string.
fn parse_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let inner = trimmed.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.to_string())
}

/// Parse a TOML-like array of double-quoted strings:
/// `["a", "b", "c"]`. Returns the inner strings in order.
/// Returns `None` if the value isn't a valid array of strings.
fn parse_string_array(value: &str) -> Option<Vec<String>> {
    let trimmed = value.trim();
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    let mut out = Vec::new();
    for piece in inner.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        out.push(parse_string(piece)?);
    }
    Some(out)
}

/// Convert a relative file path to a namespace string. Strips
/// the file extension and replaces path separators with `::`.
///
/// `"core/ffi/dload.hy"` → `"core::ffi::dload"`
/// `"foo.hy"` → `"foo"`
fn path_to_namespace(rel: &Path) -> String {
    // Strip the file extension.
    let stem = rel.with_extension("");
    // Convert path separators to `::`.
    let mut ns = String::new();
    let mut first = true;
    for component in stem.components() {
        if let std::path::Component::Normal(s) = component {
            if !first {
                ns.push_str("::");
            }
            ns.push_str(&s.to_string_lossy());
            first = false;
        }
    }
    ns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_manifest_has_src_root() {
        let m = Manifest::default();
        assert_eq!(m.roots, vec![PathBuf::from("src")]);
        assert_eq!(m.entry, None);
    }

    #[test]
    fn parse_minimal_manifest() {
        let src = "[module]\nroots = [\"./src\"]\n";
        let m = Manifest::parse(src).unwrap();
        assert_eq!(m.roots, vec![PathBuf::from("./src")]);
        assert_eq!(m.entry, None);
    }

    #[test]
    fn parse_full_manifest() {
        let src = r#"
            # coil project manifest
            [module]
            roots = ["./src", "./vendor", "./builtins"]

            [entry]
            file = "./src/main.hy"
        "#;
        let m = Manifest::parse(src).unwrap();
        assert_eq!(
            m.roots,
            vec![
                PathBuf::from("./src"),
                PathBuf::from("./vendor"),
                PathBuf::from("./builtins"),
            ]
        );
        assert_eq!(m.entry, Some(PathBuf::from("./src/main.hy")));
    }

    #[test]
    fn parse_comments_and_blank_lines() {
        let src = "# only a comment\n\n# another\n[module]\nroots = [\"./src\"] # trailing\n";
        let m = Manifest::parse(src).unwrap();
        assert_eq!(m.roots, vec![PathBuf::from("./src")]);
    }

    #[test]
    fn parse_missing_module_section_uses_default() {
        // No `[module]` section: fall back to default roots.
        let src = "[entry]\nfile = \"main.hy\"\n";
        let m = Manifest::parse(src).unwrap();
        assert_eq!(m.roots, vec![PathBuf::from("src")]);
        assert_eq!(m.entry, Some(PathBuf::from("main.hy")));
    }

    #[test]
    fn parse_invalid_kv_returns_error() {
        let src = "[module]\nthis is not a kv line\n";
        let err = Manifest::parse(src).unwrap_err();
        match err {
            ManifestError::Parse { line, .. } => assert_eq!(line, 2),
            _ => panic!("expected Parse error, got {:?}", err),
        }
    }

    #[test]
    fn parse_unknown_section_returns_error() {
        let src = "[unknown]\nfoo = \"bar\"\n";
        let err = Manifest::parse(src).unwrap_err();
        match err {
            ManifestError::Parse { line, message } => {
                assert_eq!(line, 1);
                assert!(message.contains("unknown section"));
            }
            _ => panic!("expected Parse error, got {:?}", err),
        }
    }

    #[test]
    fn path_to_namespace_strips_extension_and_uses_double_colon() {
        assert_eq!(path_to_namespace(Path::new("foo.hy")), "foo");
        assert_eq!(
            path_to_namespace(Path::new("core/ffi/dload.hy")),
            "core::ffi::dload"
        );
        assert_eq!(path_to_namespace(Path::new("a/b/c.hy")), "a::b::c");
    }

    #[test]
    fn resolve_use_finds_file_in_first_root() {
        // Build a temporary project layout:
        //   <tmp>/src/foo/sadge.hy
        // `use foo::sadge;` should resolve to that file.
        let tmp = std::env::temp_dir().join("coil_manifest_test_1");
        let src = tmp.join("src").join("foo");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("sadge.hy"), "// empty\n").unwrap();

        let m = Manifest::default(); // roots = ["src"]
        let resolved = m.resolve_use(&tmp, &["foo".into()], "sadge");
        assert!(
            resolved.is_some(),
            "expected to find sadge.hy in <tmp>/src/foo/"
        );
        let resolved = resolved.unwrap();
        assert!(resolved.ends_with("src/foo/sadge.hy"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_use_falls_back_to_second_root() {
        let tmp = std::env::temp_dir().join("coil_manifest_test_2");
        let vendor = tmp.join("vendor").join("lib_x");
        std::fs::create_dir_all(&vendor).unwrap();
        std::fs::write(vendor.join("foo.hy"), "// empty\n").unwrap();

        let m = Manifest {
            roots: vec![PathBuf::from("src"), PathBuf::from("vendor")],
            entry: None,
            ffi_search_paths: Vec::new(),
            vendor_dir: PathBuf::from("vendor"),
            dependencies: BTreeMap::new(),
        };
        let resolved = m.resolve_use(&tmp, &["lib_x".into()], "foo");
        assert!(
            resolved.is_some(),
            "expected to find foo.hy in vendor/lib_x/"
        );
        let resolved = resolved.unwrap();
        assert!(resolved.ends_with("vendor/lib_x/foo.hy"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_use_returns_none_when_missing() {
        let tmp = std::env::temp_dir().join("coil_manifest_test_3");
        std::fs::create_dir_all(&tmp).unwrap();

        let m = Manifest::default();
        let resolved = m.resolve_use(&tmp, &["nonexistent".into()], "missing");
        assert!(resolved.is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_mod_finds_top_level_file() {
        let tmp = std::env::temp_dir().join("coil_manifest_test_resolve_mod");
        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("foo.hy"), "// empty\n").unwrap();

        let m = Manifest::default();
        let resolved = m.resolve_mod(&tmp, "foo");
        assert!(resolved.is_some());
        assert!(resolved.unwrap().ends_with("src/foo.hy"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn namespace_of_returns_path_relative_to_root() {
        let tmp = std::env::temp_dir().join("coil_manifest_test_4");
        let builtins = tmp.join("builtins").join("core").join("ffi");
        std::fs::create_dir_all(&builtins).unwrap();
        let file = builtins.join("dload.hy");
        std::fs::write(&file, "// empty\n").unwrap();

        let m = Manifest {
            roots: vec![PathBuf::from("src"), PathBuf::from("builtins")],
            entry: None,
            ffi_search_paths: Vec::new(),
            vendor_dir: PathBuf::from("vendor"),
            dependencies: BTreeMap::new(),
        };
        let ns = m.namespace_of(&tmp, &file);
        assert_eq!(ns, Some("core::ffi::dload".to_string()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn namespace_of_returns_none_for_file_outside_all_roots() {
        let tmp = std::env::temp_dir().join("coil_manifest_test_5");
        let outside = tmp.join("totally").join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        let file = outside.join("x.hy");
        std::fs::write(&file, "// empty\n").unwrap();

        let m = Manifest {
            roots: vec![PathBuf::from("src")],
            entry: None,
            ffi_search_paths: Vec::new(),
            vendor_dir: PathBuf::from("vendor"),
            dependencies: BTreeMap::new(),
        };
        let ns = m.namespace_of(&tmp, &file);
        assert_eq!(ns, None);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_falls_back_to_default_when_coil_toml_absent() {
        let tmp = std::env::temp_dir().join("coil_manifest_test_6");
        std::fs::create_dir_all(&tmp).unwrap();
        // Don't create coil.toml.
        let m = Manifest::load(&tmp).unwrap();
        assert_eq!(m.roots, vec![PathBuf::from("src")]);
        assert_eq!(m.entry, None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_reads_existing_coil_toml() {
        let tmp = std::env::temp_dir().join("coil_manifest_test_7");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("coil.toml"), "[module]\nroots = [\"./vendor\"]\n").unwrap();

        let m = Manifest::load(&tmp).unwrap();
        assert_eq!(m.roots, vec![PathBuf::from("./vendor")]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn parse_package_vendor_dir_and_dependencies() {
        let src = r#"
            [package]
            vendor_dir = "third_party"

            [module]
            roots = ["./src"]

            [dependencies.foo]
            git = "https://github.com/org/foo"
            version = "^1.2.0"

            [dependencies.bar]
            git = "nina.v@example.com:org/bar.git"
            version = "~2.0"
            path = "packages/bar"
        "#;
        let m = Manifest::parse(src).unwrap();
        assert_eq!(m.vendor_dir, PathBuf::from("third_party"));
        assert_eq!(m.dependencies.len(), 2);
        let foo = m.dependencies.get("foo").unwrap();
        assert_eq!(foo.git, "https://github.com/org/foo");
        assert_eq!(foo.version, "^1.2.0");
        assert_eq!(foo.path, None);
        let bar = m.dependencies.get("bar").unwrap();
        assert_eq!(bar.path.as_deref(), Some("packages/bar"));
    }

    #[test]
    fn parse_dependency_defaults_version_to_star() {
        let src = r#"
            [dependencies.foo]
            git = "https://github.com/org/foo"
        "#;
        let m = Manifest::parse(src).unwrap();
        assert_eq!(m.dependencies["foo"].version, "*");
        assert_eq!(m.vendor_dir, PathBuf::from("vendor"));
    }

    #[test]
    fn resolve_use_finds_vendored_package_module() {
        let tmp = std::env::temp_dir().join("coil_manifest_pkg_resolve");
        let _ = std::fs::remove_dir_all(&tmp);
        let pkg = tmp.join("vendor").join("foo").join("src");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("something.hy"), "fn something() {}\n").unwrap();

        let mut deps = BTreeMap::new();
        deps.insert(
            "foo".to_string(),
            DependencySpec {
                git: "https://example.com/foo".into(),
                version: "^1".into(),
                path: None,
            },
        );
        let m = Manifest {
            roots: vec![PathBuf::from("src")],
            entry: None,
            ffi_search_paths: Vec::new(),
            vendor_dir: PathBuf::from("vendor"),
            dependencies: deps,
        };
        let resolved = m.resolve_use(&tmp, &["foo".into()], "something");
        assert!(resolved.is_some());
        assert!(resolved.unwrap().ends_with("vendor/foo/src/something.hy"));

        let ns = m.namespace_of(&tmp, &pkg.join("something.hy"));
        assert_eq!(ns.as_deref(), Some("foo::something"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn parse_rejects_duplicate_dependency_sections() {
        let src = r#"
            [dependencies.foo]
            git = "https://example.com/foo"
            [dependencies.foo]
            git = "https://example.com/foo2"
        "#;
        let err = Manifest::parse(src).unwrap_err();
        assert!(
            err.to_string().contains("duplicate dependency `foo`"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_rejects_dependency_missing_git() {
        let src = r#"
            [dependencies.foo]
            version = "^1.0"
        "#;
        let err = Manifest::parse(src).unwrap_err();
        assert!(
            err.to_string().contains("missing required `git`"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_rejects_invalid_dependency_section_name() {
        let src = r#"
            [dependencies.bad/name]
            git = "https://example.com/foo"
        "#;
        let err = Manifest::parse(src).unwrap_err();
        assert!(
            err.to_string().contains("invalid dependency section"),
            "got: {err}"
        );
    }

    #[test]
    fn resolve_use_prefers_project_root_over_vendored_package() {
        let tmp = std::env::temp_dir().join("coil_manifest_pkg_shadow");
        let _ = std::fs::remove_dir_all(&tmp);
        let local = tmp.join("src").join("foo");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("something.hy"), "// local\n").unwrap();
        let vendored = tmp.join("vendor").join("foo").join("src");
        std::fs::create_dir_all(&vendored).unwrap();
        std::fs::write(vendored.join("something.hy"), "// vendor\n").unwrap();

        let mut deps = BTreeMap::new();
        deps.insert(
            "foo".to_string(),
            DependencySpec {
                git: "https://example.com/foo".into(),
                version: "*".into(),
                path: None,
            },
        );
        let m = Manifest {
            roots: vec![PathBuf::from("src")],
            entry: None,
            ffi_search_paths: Vec::new(),
            vendor_dir: PathBuf::from("vendor"),
            dependencies: deps,
        };
        let resolved = m.resolve_use(&tmp, &["foo".into()], "something").unwrap();
        assert!(
            resolved.ends_with("src/foo/something.hy"),
            "project module roots must win over vendor packages; got {}",
            resolved.display()
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
