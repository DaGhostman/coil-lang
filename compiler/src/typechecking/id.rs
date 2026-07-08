//! Node IDs and the pre-walk that mints them.
//!
//! Phase 6 introduces a per-node cache so the bytecode emitter
//! (Phase 9) can ask "what's the inferred type of *this* AST node?"
//! without re-running inference. To avoid re-parsing or threading
//! `NodeId` through the AST, we use the following scheme:
//!
//! 1. A pre-walk visits the AST in **pre-order** (parent before
//!    children) and mints a fresh [`NodeId`] for every visit. IDs are
//!    stored in a [`Vec`] indexed by visit order.
//! 2. During inference, [`crate::typechecking::infer::Checker::infer`]
//!    pulls the next ID from the same [`Vec`] (it walks in the same
//!    order) and stores the resulting type in a
//!    `HashMap<NodeId, Ty>` cache.
//! 3. After inference, [`Checker::lookup_at`] returns the type for any
//!    ID, applying the running substitution.
//!
//! IDs are unique per AST node even when spans are shared (which
//! happens for wrapper nodes — `Program` and `Statement` covering the
//! same range, for example).

use parser::ast::{EnumConstructPayload, EnumVariantPayload, Output, Pattern, PatternPayload};

/// A stable identifier for an AST node.
///
/// IDs are minted in source order by [`pre_walk`] and reused by
/// [`crate::typechecking::infer::Checker::infer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    /// The underlying integer. Useful for pretty-printing in
    /// diagnostic messages (`#14: if ... -> bool`).
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// A sequence of [`NodeId`]s minted in pre-walk order. Each AST node
/// has exactly one entry, indexed by its position in the pre-walk.
///
/// The matching [`crate::typechecking::infer::Checker`] maintains a
/// parallel index counter so that the `n`-th call to `infer` reads
/// the `n`-th ID here.
#[derive(Debug, Default, Clone)]
pub struct IdTable {
    ids: Vec<NodeId>,
}

impl IdTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a new ID and append it to the table. Returns the new ID.
    pub fn push(&mut self) -> NodeId {
        let id = NodeId(self.ids.len() as u32);
        self.ids.push(id);
        id
    }

    /// Number of IDs minted so far.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Borrow the IDs in minting order.
    pub fn ids(&self) -> &[NodeId] {
        &self.ids
    }
}

/// Pre-walk `node` and mint a [`NodeId`] for it and every descendant.
///
/// The visit order is pre-order: a parent's ID is minted before its
/// children's. This matches [`crate::typechecking::infer::Checker::infer`]
/// (which dispatches on the parent and recurses into children), so the
/// IDs line up: every node inferred gets a cache entry, and every
/// cache ID has an inferred type.
pub fn pre_walk(node: &Output, table: &mut IdTable) {
    table.push();
    pre_walk_children(node, table);
}

fn pre_walk_children(node: &Output, table: &mut IdTable) {
    use parser::ast::Expression;
    match node.1.as_ref() {
        // No children — leaves.
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

        // Single `Output` child.
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

        // Two `Output` children.
        Expression::Assignment(name, value) => {
            pre_walk(name, table);
            pre_walk(value, table);
        }

        // Binary operators.
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

        // `(Output, Option<Vec<Output>>)` — print / format.
        Expression::Print(fmt, params) | Expression::Format(fmt, params) => {
            pre_walk(fmt, table);
            if let Some(p) = params {
                for param in p {
                    pre_walk(param, table);
                }
            }
        }

        // `(Output, Option<Output>)` — resume.
        Expression::Resume(target, arg) => {
            pre_walk(target, table);
            if let Some(a) = arg {
                pre_walk(a, table);
            }
        }

        // `Vec<Output>` — block, program, fragment, list,
        // and the userland FFI builtins (`declare`/`invoke`
        // carry a list of args).
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
        // `dload(path)` — single child.
        Expression::Dload(path) => pre_walk(path, table),
        // `(a, b, c)` — tuple literal. Walks each element so
        // each gets its own NodeId for opcode selection.
        Expression::Tuple(items) => {
            for c in items {
                pre_walk(c, table);
            }
        }
        // `[a, b, c]` — array literal. Same as Tuple for ID
        // walking purposes (the runtime distinguishes by
        // opcode, not by AST shape).
        Expression::Array(items) => {
            for c in items {
                pre_walk(c, table);
            }
        }
        // `t[i]` — index access. Walks both operands.
        Expression::Index(target, index) => {
            pre_walk(target, table);
            pre_walk(index, table);
        }
        // `{ name: expr, ... }` — dict literal. Walks every
        // value expression (the field NAME is inert metadata
        // and doesn't need an ID).
        Expression::Dict(fields) => {
            for f in fields {
                pre_walk(&f.value, table);
            }
        }
        // `If(branches)` — each branch is itself an Output.
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

        // Function: args + body.
        Expression::Function { args, body, .. } => {
            pre_walk(args, table);
            pre_walk(body, table);
        }

        // Branch: optional cond + body.
        Expression::Branch(cond, body) => {
            if let Some(c) = cond {
                pre_walk(c, table);
            }
            pre_walk(body, table);
        }

        // Call: name + optional args.
        Expression::Call { name, args } => {
            pre_walk(name, table);
            if let Some(a) = args {
                for arg in a {
                    pre_walk(arg, table);
                }
            }
        }

        // Loop: iterable + body + optional identifier.
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

        // Match: scrutinee + arms. Each arm carries a pattern
        // (which is *not* an expression and has no NodeId) and a
        // body (which is a normal `Output`). The pre-walk visits
        // the body of each arm so the infer pass can keep its
        // NodeId counter in lockstep; the pattern is visited by
        // [`pre_walk_pattern`] for structural traversal without
        // minting IDs.
        Expression::Match { scrutinee, arms } => {
            pre_walk(scrutinee, table);
            for arm in arms {
                pre_walk_pattern(&arm.pattern, table);
                pre_walk(&arm.body, table);
            }
        }

        // ---- Phase 15A: sum types and constructors ----
        // Phase 17B: `enum_decl` carries a list of `EnumVariant`
        // outputs (the variants). The pre-walk visits each
        // variant's output, which dispatches to the `EnumVariant`
        // arm below.
        Expression::EnumDecl { variants, .. } => {
            for v in variants {
                pre_walk(v, table);
            }
        }
        // Phase 28 — `type Name = T;` aliases. Walk the RHS
        // so its children consume IDs in lockstep with the
        // pre-walk. Alias resolution happens in the
        // typechecker (substituting Name with the RHS Ty on
        // lookup), not in the pre-walk.
        Expression::TypeAlias { ty, .. } => {
            pre_walk(ty, table);
        }
        // FFI declaration block: each function declaration has
        // its own `Output` (the `args` field). The pre-walk
        // visits each so the typechecker's ID cache lines up
        // with the codegen's ID consumption. The function
        // arguments are visited as `Argument` outputs.
        Expression::ExternBlock { declarations, .. } => {
            for decl in declarations {
                pre_walk(&decl.args, table);
            }
        }
        // `EnumVariant` carries the payload shape (`EnumVariantPayload`):
        // - Unit: no children.
        // - Tuple: each element is a `Output` (typically
        //   `Expression::Type(...)`); pre-walk each so the cache
        //   lines up.
        // - Record: each field's `value` is a `Output` (typically
        //   `Expression::Type(...)`); pre-walk each. The field's
        //   `name` is just a string — no NodeId.
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
        // `Construct` carries the application shape
        // (`EnumConstructPayload`). Unit has no children. Tuple /
        // Record each carry `Output` sub-expressions that need
        // NodeIds (so the infer pass can type-check them).
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

        // Method: body (visibility is metadata, no AST to recurse).
        Expression::Method(_, body) => pre_walk(body, table),

        // Member access: receiver only (field name is metadata).
        Expression::Access(receiver, _) => pre_walk(receiver, table),

        // Instantiation: class + optional args.
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

/// Structural walk over a [`Pattern`] tree.
///
/// Patterns are not expressions and do not carry a [`NodeId`] — they
/// don't produce a `Ty`. The HM infer pass handles patterns
/// separately (via `Checker::infer_pattern`, land in 15B). The
/// pre-walk still needs to visit every nested pattern so the
/// structural invariants of the AST are explored (and so any future
/// pattern-level analysis — exhaustiveness checking, for instance —
/// can rely on a consistent walk).
///
/// `Wildcard` and `Binding` are leaves. `Constructor` recurses into
/// each sub-pattern in the payload. Phase 17B: payload is
/// `PatternPayload` (Unit / Tuple / Record). Record patterns have
/// `PatternField` entries — the field `pattern` recurses, but the
/// field name is just a string (no NodeId).
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
        // `1 + 2;` wraps in Program / Statement / ExprStatement /
        // Expr / Add / Integer / Integer — 7 nodes total.
        assert_eq!(count_nodes("1 + 2;"), 7);
    }

    #[test]
    fn pre_walk_assigns_unique_ids_in_visit_order() {
        let ast = Pratt::default().parse("42;").expect("parse failed");
        let mut table = IdTable::new();
        pre_walk(&ast, &mut table);
        let ids = table.ids();
        // Every consecutive pair of IDs is distinct.
        for pair in ids.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
        // IDs are 0..N.
        assert_eq!(ids[0], NodeId(0));
        assert_eq!(ids[ids.len() - 1], NodeId((ids.len() - 1) as u32));
    }

    #[test]
    fn pre_walk_handles_shared_spans_with_distinct_ids() {
        // Program and Statement share a span but are different AST
        // nodes. The pre-walk must give them different IDs.
        let ast = Pratt::default().parse("42;").expect("parse failed");
        let mut table = IdTable::new();
        pre_walk(&ast, &mut table);
        // At least 3 IDs minted (Program, Statement, ExprStatement, Expr, Integer).
        assert!(table.len() >= 3);
        // IDs are unique (no duplicates).
        let mut seen = std::collections::HashSet::new();
        for id in table.ids() {
            assert!(seen.insert(*id), "duplicate ID: {:?}", id);
        }
    }
}
