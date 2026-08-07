//! Project-wide symbol index backed by the compiler pipeline and HM checker.

use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
};

use reporting::Message;

use crate::{
    Checker, Manifest, Pipeline, SymbolDef, SymbolIndex, SymbolKind,
};

#[derive(Clone)]
struct IndexedFile {
    source: String,
    symbols: SymbolIndex,
}

/// Disk-backed project index wrapping [`Pipeline`] typechecking state.
pub struct ProjectIndex {
    pipeline: Pipeline,
    project_root: PathBuf,
    files: HashMap<PathBuf, IndexedFile>,
}

impl ProjectIndex {
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            pipeline: Pipeline::new(),
            project_root,
            files: HashMap::new(),
        }
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn pipeline(&self) -> &Pipeline {
        &self.pipeline
    }

    pub fn pipeline_mut(&mut self) -> &mut Pipeline {
        &mut self.pipeline
    }

    pub fn checker(&self) -> &Checker {
        self.pipeline.compiler().checker()
    }

    /// Walk `coil.toml` module roots and index every `.hy` file on disk.
    pub fn index_from_manifest(&mut self) {
        let manifest = Manifest::load(&self.project_root).unwrap_or_default();
        let mut paths = Vec::new();
        for root in &manifest.roots {
            let abs = self.project_root.join(root);
            collect_hy_files(&abs, &mut paths);
        }
        paths.sort();
        paths.dedup();
        for path in paths {
            if let Ok(source) = std::fs::read_to_string(&path) {
                self.upsert_file(path, source);
            }
        }
    }

    /// Typecheck the module graph from `entry` and refresh indexed sources.
    pub fn typecheck_entry(&mut self, entry: &Path) -> Vec<(PathBuf, Vec<Message>)> {
        let results = self.pipeline.typecheck_project(entry);
        for (path, _) in &results {
            if let Ok(source) = std::fs::read_to_string(path) {
                self.upsert_file(path.clone(), source);
            }
        }
        results
    }

    pub fn upsert_file(&mut self, path: PathBuf, source: String) {
        let symbols = SymbolIndex::from_source(path.clone(), &source);
        self.files.insert(path, IndexedFile { source, symbols });
    }

    pub fn source_for(&self, path: &Path) -> Option<&str> {
        self.files.get(path).map(|f| f.source.as_str())
    }

    pub fn symbols_for(&self, path: &Path) -> Option<&SymbolIndex> {
        self.files.get(path).map(|f| &f.symbols)
    }

    /// Resolve a reference site to definition locations using checker disambiguation.
    pub fn resolve_definition(
        &self,
        file: &Path,
        ref_range: Range<usize>,
        name: &str,
    ) -> Vec<(PathBuf, Range<usize>)> {
        let checker = self.checker();
        let index = self.files.get(file).map(|f| &f.symbols);

        if let Some(_index) = index {
            if checker
                .bound_method_call_for_span(ref_range.start, ref_range.end)
                .is_some()
            {
                let methods: Vec<_> = self
                    .files
                    .values()
                    .flat_map(|f| f.symbols.definitions(name))
                    .filter(|def| matches!(def.kind, SymbolKind::Method | SymbolKind::Function))
                    .map(|def| (def.file.clone(), def.name_range.clone()))
                    .collect();
                if !methods.is_empty() {
                    return methods;
                }
            }

            if checker
                .selected_overload_at(ref_range.start, ref_range.end)
                .is_some()
            {
                let fns: Vec<_> = self
                    .files
                    .values()
                    .flat_map(|f| f.symbols.definitions(name))
                    .filter(|def| matches!(def.kind, SymbolKind::Function | SymbolKind::Method))
                    .map(|def| (def.file.clone(), def.name_range.clone()))
                    .collect();
                if fns.len() == 1 {
                    return fns;
                }
            }

            if checker
                .lookup_for_codegen_span(ref_range.start, ref_range.end)
                .is_some()
            {
                let typed: Vec<_> = self
                    .files
                    .values()
                    .flat_map(|f| f.symbols.definitions(name))
                    .filter(|def| {
                        !matches!(
                            def.kind,
                            SymbolKind::Namespace | SymbolKind::TypeAlias
                        ) || checker.env().lookup(name).is_some()
                    })
                    .map(|def| (def.file.clone(), def.name_range.clone()))
                    .collect();
                if typed.len() == 1 {
                    return typed;
                }
            }
        }

        self.files
            .values()
            .flat_map(|f| f.symbols.definitions(name))
            .map(|def: &SymbolDef| (def.file.clone(), def.name_range.clone()))
            .collect()
    }
}

fn collect_hy_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_hy_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "hy") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn upsert_indexes_function_definition() {
        let dir = std::env::temp_dir().join(format!("coil-project-index-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.hy");
        let src = "fn helper() -> int { return 1; }\nfn main() { let x = helper(); }\n";
        fs::write(&path, src).unwrap();

        let mut index = ProjectIndex::new(dir.clone());
        index.upsert_file(path.clone(), src.to_string());
        let symbols = index.symbols_for(&path).expect("indexed");
        let defs = symbols.definitions("helper");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].kind, crate::SymbolKind::Function);

        let refs = symbols.references("helper");
        assert!(!refs.is_empty(), "expected a reference site for helper()");
        let hit = &refs[0];
        let resolved = index.resolve_definition(&path, hit.range.clone(), "helper");
        assert!(
            resolved.iter().any(|(f, r)| f == &path && *r == defs[0].name_range),
            "resolve_definition should return helper's def, got: {resolved:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_replaces_stale_source() {
        let dir = std::env::temp_dir().join(format!("coil-project-index-upd-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.hy");

        let mut index = ProjectIndex::new(dir.clone());
        index.upsert_file(path.clone(), "fn old() {}\n".into());
        assert!(index.symbols_for(&path).unwrap().definitions("old").len() == 1);

        index.upsert_file(path.clone(), "fn neu() {}\n".into());
        let symbols = index.symbols_for(&path).unwrap();
        assert!(symbols.definitions("old").is_empty());
        assert_eq!(symbols.definitions("neu").len(), 1);
        assert_eq!(index.source_for(&path), Some("fn neu() {}\n"));

        let _ = fs::remove_dir_all(&dir);
    }
}
