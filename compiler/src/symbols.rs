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
