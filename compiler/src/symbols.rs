use std::{
    collections::HashMap,
    ops::Range,
    path::PathBuf,
};

use parser::ast::{Expression, Output};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Variable,
    Class,
    Enum,
    TypeAlias,
    Namespace,
    Method,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolDef {
    pub name: String,
    pub kind: SymbolKind,
    pub file: PathBuf,
    pub range: Range<usize>,
    pub name_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefSite {
    pub name: String,
    pub file: PathBuf,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct SymbolIndex {
    definitions: HashMap<String, Vec<SymbolDef>>,
    references: HashMap<String, Vec<RefSite>>,
}

impl SymbolIndex {
    pub fn from_source(file: PathBuf, source: &str) -> Self {
        let Ok(root) = parser::Pratt::default().parse(source) else {
            return Self::default();
        };
        let mut index = Self::default();
        index.collect_definitions(&file, source, &root);
        index.collect_references(&file, &root);
        index
    }

    pub fn definitions(&self, name: &str) -> &[SymbolDef] {
        self.definitions.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn references(&self, name: &str) -> &[RefSite] {
        self.references.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn all_definitions(&self) -> impl Iterator<Item = &SymbolDef> {
        self.definitions.values().flatten()
    }

    fn collect_definitions(&mut self, file: &PathBuf, source: &str, expression: &Output<'_>) {
        let Expression::Program(items) = expression.1.as_ref() else {
            return;
        };
        for item in items {
            let (span, expression) = item;
            let (name, kind) = match expression.as_ref() {
                Expression::Function { name, .. } => (*name, SymbolKind::Function),
                Expression::Class { name, .. } => (*name, SymbolKind::Class),
                Expression::EnumDecl { name, .. } => (*name, SymbolKind::Enum),
                Expression::TypeAlias { name, .. } => (*name, SymbolKind::TypeAlias),
                Expression::StaticDecl { name, .. } => (*name, SymbolKind::Variable),
                Expression::AttrDecl { name, .. } => (*name, SymbolKind::Method),
                Expression::Use { name, alias, .. } => (
                    alias.as_deref().unwrap_or(name),
                    SymbolKind::Namespace,
                ),
                _ => continue,
            };
            let range = span.start..span.end;
            let name_start = source[range.clone()]
                .find(name)
                .map(|offset| range.start + offset)
                .unwrap_or(range.start);
            let definition = SymbolDef {
                name: name.to_owned(),
                kind,
                file: file.clone(),
                range,
                name_range: name_start..name_start + name.len(),
            };
            self.definitions
                .entry(name.to_owned())
                .or_default()
                .push(definition);
        }
    }

    fn collect_references(&mut self, file: &PathBuf, expression: &Output<'_>) {
        fn visit(index: &mut SymbolIndex, file: &PathBuf, expression: &Expression<'_>, span: Range<usize>) {
            match expression {
                Expression::Identifier(name) => {
                    index
                        .references
                        .entry((*name).to_owned())
                        .or_default()
                        .push(RefSite {
                            name: (*name).to_owned(),
                            file: file.clone(),
                            range: span,
                        });
                }
                Expression::Program(items)
                | Expression::Block(items)
                | Expression::Fragment(items) => {
                    for (child_span, child) in items {
                        visit(index, file, child, child_span.start..child_span.end);
                    }
                }
                Expression::Function { args, body, .. } => {
                    visit(index, file, &args.1, args.0.start..args.0.end);
                    if let Some(body) = body {
                        visit(index, file, &body.1, body.0.start..body.0.end);
                    }
                }
                Expression::Call { name, args } => {
                    visit(index, file, &name.1, name.0.start..name.0.end);
                    if let Some(args) = args {
                        for arg in args {
                            visit(index, file, &arg.1, arg.0.start..arg.0.end);
                        }
                    }
                }
                _ => {}
            }
        }
        visit(self, file, &expression.1, expression.0.start..expression.0.end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn index(source: &str) -> SymbolIndex {
        SymbolIndex::from_source(PathBuf::from("test.hy"), source)
    }

    #[test]
    fn indexes_top_level_definition_kinds() {
        let source = "\
use io::stdout as out;
type Id = int;
static let hits = 0;
enum Color { Red, Green }
class Point { pub x: int, pub y: int }
fn add(int a, int b) -> int { return a + b; }
";
        let idx = index(source);

        let out = idx.definitions("out");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, SymbolKind::Namespace);

        let id = idx.definitions("Id");
        assert_eq!(id.len(), 1);
        assert_eq!(id[0].kind, SymbolKind::TypeAlias);

        let hits = idx.definitions("hits");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, SymbolKind::Variable);

        let color = idx.definitions("Color");
        assert_eq!(color.len(), 1);
        assert_eq!(color[0].kind, SymbolKind::Enum);

        let point = idx.definitions("Point");
        assert_eq!(point.len(), 1);
        assert_eq!(point[0].kind, SymbolKind::Class);

        let add = idx.definitions("add");
        assert_eq!(add.len(), 1);
        assert_eq!(add[0].kind, SymbolKind::Function);
        assert_eq!(&source[add[0].name_range.clone()], "add");
    }

    #[test]
    fn indexes_call_identifier_references() {
        let source = "\
fn fib(int n) -> int {
    return fib(n - 1);
}
fn main() {
    fib(10);
    return;
}
";
        let idx = index(source);
        assert_eq!(idx.definitions("fib").len(), 1);
        let refs = idx.references("fib");
        assert!(
            refs.len() >= 2,
            "expected recursive + main call sites, got {refs:?}"
        );
        for site in refs {
            assert_eq!(&source[site.range.clone()], "fib");
        }
    }

    #[test]
    fn parse_failure_yields_empty_index() {
        let idx = index("fn {{{");
        assert!(idx.all_definitions().next().is_none());
        assert!(idx.definitions("anything").is_empty());
        assert!(idx.references("anything").is_empty());
    }

    #[test]
    fn use_without_alias_keeps_imported_name() {
        let idx = index("use io::stdout;\nfn main() { return; }\n");
        let defs = idx.definitions("stdout");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].kind, SymbolKind::Namespace);
    }
}
