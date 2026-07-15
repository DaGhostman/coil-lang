//! Pre-walk [`NodeId`] minting for span-indexed type lookup.
//!
//! The pre-walk and [`Checker::infer`](super::infer::Checker::infer) both
//! visit the AST in pre-order, so the n-th infer call consumes the n-th ID.

use parser::ast::{EnumConstructPayload, EnumVariantPayload, Output, Pattern, PatternPayload};

/// Stable identifier for an AST node (minted in pre-walk visit order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// IDs minted in pre-walk order; consumed in lockstep by inference.
#[derive(Debug, Default, Clone)]
pub struct IdTable {
    ids: Vec<NodeId>,
}

impl IdTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self) -> NodeId {
        let id = NodeId(self.ids.len() as u32);
        self.ids.push(id);
        id
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn ids(&self) -> &[NodeId] {
        &self.ids
    }
}

/// Pre-order walk: mint one ID per node, then recurse into children.
pub fn pre_walk(node: &Output, table: &mut IdTable) {
    table.push();
    pre_walk_children(node, table);
}

fn pre_walk_children(node: &Output, table: &mut IdTable) {
    use parser::ast::Expression;
    match node.1.as_ref() {
        Expression::Noop(_)
        | Expression::Comment(_)
        | Expression::Integer(_)
        | Expression::Float(_)
        | Expression::String(_)
        | Expression::Bool(_)
        | Expression::Identifier(_)
        | Expression::Type(_)
        | Expression::Default(_)
        | Expression::Use { .. }
        | Expression::Module(_, _)
        | Expression::Variable(_, _)
        | Expression::Constant(_, _)
        | Expression::Argument(_, _)
        | Expression::Field(_, _, _) => {}

        Expression::Expr(e)
        | Expression::Group(e)
        | Expression::Statement(e)
        | Expression::ExprStatement(e)
        | Expression::Return(e)
        | Expression::ImplicitReturn(e)
        | Expression::Yield(e)
        | Expression::Negate(e)
        | Expression::Not(e)
        | Expression::Positive(e)
        | Expression::Inc(e)
        | Expression::Dec(e)
        | Expression::Defer(e)
        | Expression::Member(e)
        | Expression::Update(_, e) => pre_walk(e, table),

        Expression::Assignment(name, value) => {
            pre_walk(name, table);
            pre_walk(value, table);
        }

        Expression::Add(l, r)
        | Expression::Sub(l, r)
        | Expression::Mul(l, r)
        | Expression::Div(l, r)
        | Expression::Mod(l, r)
        | Expression::Pow(l, r)
        | Expression::Shl(l, r)
        | Expression::Shr(l, r)
        | Expression::Xor(l, r)
        | Expression::And(l, r)
        | Expression::Or(l, r)
        | Expression::BitAnd(l, r)
        | Expression::BitOr(l, r)
        | Expression::Eq(l, r)
        | Expression::Neq(l, r)
        | Expression::Le(l, r)
        | Expression::Gt(l, r)
        | Expression::Leq(l, r)
        | Expression::Geq(l, r) => {
            pre_walk(l, table);
            pre_walk(r, table);
        }

        Expression::Print(fmt, params) | Expression::Format(fmt, params) => {
            pre_walk(fmt, table);
            if let Some(p) = params {
                for param in p {
                    pre_walk(param, table);
                }
            }
        }

        Expression::Resume(target, arg) => {
            pre_walk(target, table);
            if let Some(a) = arg {
                pre_walk(a, table);
            }
        }

        Expression::Block(cs)
        | Expression::Program(cs)
        | Expression::Fragment(cs)
        | Expression::List(cs)
        | Expression::Declare(cs)
        | Expression::Invoke(cs) => {
            for c in cs {
                pre_walk(c, table);
            }
        }
        Expression::Dload(path) => pre_walk(path, table),
        Expression::Tuple(items) => {
            for c in items {
                pre_walk(c, table);
            }
        }
        Expression::Array(items) => {
            for c in items {
                pre_walk(c, table);
            }
        }
        Expression::Index(target, index) => {
            pre_walk(target, table);
            pre_walk(index, table);
        }
        Expression::Dict(fields) => {
            for f in fields {
                pre_walk(&f.value, table);
            }
        }
        Expression::If(branches) => {
            for b in branches {
                pre_walk(b, table);
            }
        }
        Expression::Implementation(_, _, methods) => {
            for m in methods {
                pre_walk(m, table);
            }
        }
        Expression::Class(_, fields) => {
            for f in fields {
                pre_walk(f, table);
            }
        }

        Expression::Function { args, body, .. } => {
            pre_walk(args, table);
            pre_walk(body, table);
        }

        Expression::Branch(cond, body) => {
            if let Some(c) = cond {
                pre_walk(c, table);
            }
            pre_walk(body, table);
        }

        Expression::Call { name, args } => {
            pre_walk(name, table);
            if let Some(a) = args {
                for arg in a {
                    pre_walk(arg, table);
                }
            }
        }

        Expression::Loop {
            iterable,
            body,
            identifier,
        } => {
            pre_walk(iterable, table);
            pre_walk(body, table);
            if let Some(i) = identifier {
                pre_walk(i, table);
            }
        }

        // Patterns have no NodeId; walk bodies only (lockstep with infer).
        Expression::Match { scrutinee, arms } => {
            pre_walk(scrutinee, table);
            for arm in arms {
                pre_walk_pattern(&arm.pattern, table);
                pre_walk(&arm.body, table);
            }
        }

        Expression::EnumDecl { variants, .. } => {
            for v in variants {
                pre_walk(v, table);
            }
        }
        Expression::TypeAlias { ty, .. } => {
            pre_walk(ty, table);
        }
        Expression::ExternBlock { declarations, .. } => {
            for decl in declarations {
                pre_walk(&decl.args, table);
            }
        }
        Expression::EnumVariant { payload, .. } => match payload {
            EnumVariantPayload::Unit => {}
            EnumVariantPayload::Tuple(parts) => {
                for p in parts {
                    pre_walk(p, table);
                }
            }
            EnumVariantPayload::Record(fields) => {
                for f in fields {
                    pre_walk(&f.value, table);
                }
            }
        },
        Expression::Construct { fields, .. } => match fields {
            EnumConstructPayload::Unit => {}
            EnumConstructPayload::Tuple(args) => {
                for arg in args {
                    pre_walk(arg, table);
                }
            }
            EnumConstructPayload::Record(parts) => {
                for p in parts {
                    pre_walk(&p.value, table);
                }
            }
        },

        Expression::Method(_, body) => pre_walk(body, table),

        Expression::Access(receiver, _) => pre_walk(receiver, table),

        Expression::Instantiate(class, args) => {
            pre_walk(class, table);
            if let Some(a) = args {
                for arg in a {
                    pre_walk(arg, table);
                }
            }
        }
    }
}

/// Structural walk over patterns (no NodeIds).
pub fn pre_walk_pattern(pattern: &Pattern, _table: &mut IdTable) {
    match pattern {
        Pattern::Wildcard | Pattern::Binding { .. } => {}
        Pattern::Constructor { payload, .. } => match payload {
            PatternPayload::Unit => {}
            PatternPayload::Tuple(parts) => {
                for p in parts {
                    pre_walk_pattern(p, _table);
                }
            }
            PatternPayload::Record(fields) => {
                for pf in fields {
                    pre_walk_pattern(&pf.pattern, _table);
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parser::Pratt;

    fn count_nodes(src: &str) -> usize {
        let ast = Pratt::default().parse(src).expect("parse failed");
        let mut table = IdTable::new();
        pre_walk(&ast, &mut table);
        table.len()
    }

    #[test]
    fn pre_walk_mints_one_id_per_node_for_simple_expr() {
        assert_eq!(count_nodes("1 + 2;"), 7);
    }

    #[test]
    fn pre_walk_assigns_unique_ids_in_visit_order() {
        let ast = Pratt::default().parse("42;").expect("parse failed");
        let mut table = IdTable::new();
        pre_walk(&ast, &mut table);
        let ids = table.ids();
        for pair in ids.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
        assert_eq!(ids[0], NodeId(0));
        assert_eq!(ids[ids.len() - 1], NodeId((ids.len() - 1) as u32));
    }

    #[test]
    fn pre_walk_handles_shared_spans_with_distinct_ids() {
        let ast = Pratt::default().parse("42;").expect("parse failed");
        let mut table = IdTable::new();
        pre_walk(&ast, &mut table);
        assert!(table.len() >= 3);
        let mut seen = std::collections::HashSet::new();
        for id in table.ids() {
            assert!(seen.insert(*id), "duplicate ID: {:?}", id);
        }
    }
}
