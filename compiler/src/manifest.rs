//! Project manifest (`coil.toml`) parsing and module path resolution.
//!
//! A `coil.toml` at the project root declares search roots for `use`
//! resolution and an optional entry point. The pipeline maps `a::b::c`
//! paths to `<root>/a/b/c.hy` files on disk.
//!
//! ## Format
//!
//! ```toml
//! [module]
//! roots = ["./src", "./vendor", "./builtins"]
//!
//! [entry]
//! # Optional. Default = the file passed to the compiler.
//! file = "./src/main.hy"
//! ```
//!
//! The parser is intentionally minimal (no nested tables,
//! no inline tables, no arrays of tables). The grammar is:
//!
//! ```text
//! file   := section* ; zero or more sections
//! section := '[' ident ']' '\n' (entry '\n')*
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
//! The first match wins. If no root contains the file, the
//! pipeline emits a "module not found" diagnostic.

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
}

impl Default for Manifest {
    /// Default manifest when no `coil.toml` is present:
    /// a single search root at `src/`, no explicit entry
    /// point.
    fn default() -> Self {
        Self {
            roots: vec![PathBuf::from("src")],
            entry: None,
            ffi_search_paths: Vec::new(),
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
        let mut current_section: Option<&'static str> = None;

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

            // Section header: `[name]`.
            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                current_section = match name.trim() {
                    "module" => Some("module"),
                    "entry" => Some("entry"),
                    "ffi" => Some("ffi"),
                    other => {
                        return Err(ManifestError::Parse {
                            line: line_num,
                            message: format!("unknown section `[{}]`", other),
                        });
                    }
                };
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

            match (section, key) {
                ("module", "roots") => {
                    let parsed = parse_string_array(value).ok_or(ManifestError::Parse {
                        line: line_num,
                        message: format!("expected array of strings, got `{}`", value),
                    })?;
                    roots = Some(parsed.into_iter().map(PathBuf::from).collect());
                }
                ("entry", "file") => {
                    let parsed = parse_string(value).ok_or(ManifestError::Parse {
                        line: line_num,
                        message: format!("expected string, got `{}`", value),
                    })?;
                    entry = Some(PathBuf::from(parsed));
                }
                ("ffi", "search_paths") => {
                    let parsed = parse_string_array(value).ok_or(ManifestError::Parse {
                        line: line_num,
                        message: format!("expected array of strings, got `{}`", value),
                    })?;
                    ffi_search_paths = Some(parsed.into_iter().map(PathBuf::from).collect());
                }
                (section, key) => {
                    return Err(ManifestError::Parse {
                        line: line_num,
                        message: format!("unknown key `{}.{}`", section, key),
                    });
                }
            }
        }

        Ok(Self {
            roots: roots.unwrap_or_else(|| vec![PathBuf::from("src")]),
            entry,
            ffi_search_paths: ffi_search_paths.unwrap_or_default(),
        })
    }

    /// Resolve a `use` target (`a::b::c`) to an absolute file
    /// path. Searches each search root in order; the first
    /// match wins. Returns `None` if no root contains the
    /// module file.
    ///
    /// `path` is the segments of the module path BEFORE the
    /// item name (e.g. `["a", "b"]` for `use a::b::c;`).
    /// `name` is the final segment (e.g. `"c"`).
    ///
    /// The file containing the module is the LAST
    /// segment of the dotted path (as a file stem). For
    /// `use foo::sadge;`, the file is `foo.hy` (NOT
    /// `foo/sadge.hy`). For `use lib::io::read;`, the
    /// file is `io.hy` inside `lib/` (so the full path
    /// is `<root>/lib/io.hy`).
    ///
    /// The fully qualified name of the imported item is
    /// `<file's path>::<name>` — the file's directory
    /// path is the namespace, and the function name is
    /// the LAST segment. So `sadge` in `foo.hy` has
    /// FQN `foo::sadge`, and `read` in `lib/io.hy` has
    /// FQN `lib::io::read`.
    pub fn resolve_use(&self, project_root: &Path, path: &[String], name: &str) -> Option<PathBuf> {
        for root in &self.roots {
            let mut candidate = project_root.join(root);
            for segment in path {
                candidate.push(segment);
            }
            // The file is `name.hy` inside the directory
            // `<root>/<path joined>`. So for `use
            // foo::sadge;`, file = `<root>/foo/sadge.hy`.
            // For `use lib::io::read;`, file =
            // `<root>/lib/io/read.hy`.
            candidate.push(format!("{}.hy", name));
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
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
    /// path and the project root. The namespace is the path
    /// of the file relative to the FIRST search root that
    /// contains it, with the file extension stripped and
    /// path separators replaced with `::`.
    ///
    /// For example, given roots `["./src", "./builtins"]` and
    /// file `./builtins/core/ffi/dload.hy`, the namespace is
    /// `core::ffi::dload`.
    ///
    /// Returns `None` if the file is not inside any search
    /// root. Files outside any search root are still
    /// compilable (we use their bare stem as the namespace),
    /// but the caller is expected to handle that fallback.
    pub fn namespace_of(&self, project_root: &Path, file: &Path) -> Option<String> {
        for root in &self.roots {
            let abs_root = project_root.join(root);
            if let Ok(rel) = file.strip_prefix(&abs_root) {
                return Some(path_to_namespace(rel));
            }
        }
        None
    }
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
}
