//! Static shape + profitability analysis for Independent Parallel Arms (IPA).
//!
//! A *fork site* is an expression whose operands are two or more mutually
//! independent self-calls: `f(a) ⊕ f(b)`, `E::V(f(a), f(b))`, or the tak-style
//! `f(f(a), f(b), f(c))`. Arms are described structurally ([`ArgForm`]) rather
//! than by function allowlists, so any arity and any combine shape below is
//! recognized. Call sites with constant args above [`par_cost_threshold`] are
//! rewritten to specialized nullary clones that always fork (fully static, no
//! runtime threshold checks).

use std::collections::{BTreeSet, HashMap, HashSet};

use parser::ast::{EnumConstructPayload, Expression, Output};

use super::purity::RecursivePureSet;

/// Compile-time fork threshold (`COIL_PAR_THRESHOLD`, default 20).
pub fn par_cost_threshold() -> i64 {
    static T: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *T.get_or_init(|| {
        std::env::var("COIL_PAR_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(20)
    })
}

/// Binary op used at a [`ParCombine::BinOp`] fork site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParBinOp {
    Add,
    Sub,
    Mul,
}

/// One argument of a self-call arm, expressed in terms of the enclosing
/// function's parameters so child arg vectors can be derived statically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArgForm {
    /// Literal integer.
    Const(i64),
    /// Parameter forwarded unchanged (`x`).
    Param(usize),
    /// `param - sub` with `sub > 0`; requires an int-like parameter.
    ParamMinus { param: usize, sub: i64 },
}

/// One independent arm of a fork site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParArm {
    /// Call to the enclosing function; one [`ArgForm`] per parameter.
    SelfCall { args: Vec<ArgForm> },
}

/// How arm results are recombined once every arm has been joined.
///
/// The combine always consumes the arm results positionally and in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParCombine {
    /// `arm0 ⊕ arm1` (exactly two arms).
    BinOp(ParBinOp),
    /// Rebuild by calling the same fn with the arm results as args (tak-style).
    SelfCall,
    /// `EnumName::Variant(arm0, arm1, …)`.
    EnumCtor {
        enum_name: String,
        variant_name: String,
    },
}

/// A recursive-pure function's primary parallelizable fork site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParForkSite {
    pub fn_name: String,
    /// Declared parameter count; every arm supplies exactly this many args.
    pub param_count: usize,
    /// Independent self-calls (at least two).
    pub arms: Vec<ParArm>,
    pub combine: ParCombine,
}

/// True when a concrete arg vector is expensive enough to fork.
///
/// Cost is approximated by the largest argument; empty vectors never fork.
pub fn const_args_worth_parallel(args: &[i64]) -> bool {
    args.iter()
        .copied()
        .max()
        .is_some_and(|m| m > par_cost_threshold())
}

/// Specialized nullary entry name for `fn_name` at concrete `args`.
///
/// `("fib", &[22])` → `__coil_par_fib_22`; `("tak", &[18, 12, 6])` →
/// `__coil_par_tak_18_12_6`.
pub fn par_specialization_name(fn_name: &str, args: &[i64]) -> String {
    let mut out = format!("__coil_par_{fn_name}");
    for a in args {
        out.push('_');
        out.push_str(&a.to_string());
    }
    out
}

/// Concrete child args for `arm` given the enclosing call's `parent_args`.
pub fn eval_arm_args(arm: &ParArm, parent_args: &[i64]) -> Option<Vec<i64>> {
    let ParArm::SelfCall { args } = arm;
    args.iter().map(|f| eval_arg_form(f, parent_args)).collect()
}

fn eval_arg_form(form: &ArgForm, parent_args: &[i64]) -> Option<i64> {
    match form {
        ArgForm::Const(k) => Some(*k),
        ArgForm::Param(i) => parent_args.get(*i).copied(),
        ArgForm::ParamMinus { param, sub } => parent_args.get(*param).map(|v| v - sub),
    }
}

/// Collect the primary fork site of every recursive-pure function.
///
/// One site per function: the first profitable one found walking the body,
/// preferring sites on a `return` / implicit-return path.
pub fn analyze_par_fork_sites(
    ast: &Output<'_>,
    recursive_pure: &RecursivePureSet,
) -> HashMap<String, ParForkSite> {
    let mut out = HashMap::new();
    collect_sites(ast, recursive_pure, &mut out);
    out
}

/// Constant call-site arg vectors for fork-site functions, closed under the
/// arm transforms so every specialization a clone can reach also exists.
pub fn collect_par_specialization_args(
    ast: &Output<'_>,
    sites: &HashMap<String, ParForkSite>,
) -> HashMap<String, BTreeSet<Vec<i64>>> {
    let mut demanded: HashMap<String, BTreeSet<Vec<i64>>> = HashMap::new();
    collect_const_calls(ast, sites, &mut demanded);
    for (name, set) in demanded.iter_mut() {
        let Some(site) = sites.get(name) else {
            continue;
        };
        let mut stack: Vec<Vec<i64>> = set.iter().cloned().collect();
        let mut seen: HashSet<Vec<i64>> = HashSet::new();
        while let Some(cur) = stack.pop() {
            if !seen.insert(cur.clone()) {
                continue;
            }
            for arm in &site.arms {
                let Some(child) = eval_arm_args(arm, &cur) else {
                    continue;
                };
                if const_args_worth_parallel(&child) && !seen.contains(&child) {
                    stack.push(child);
                }
            }
            set.insert(cur);
        }
    }
    demanded.retain(|_, set| {
        set.retain(|args| const_args_worth_parallel(args));
        !set.is_empty()
    });
    demanded
}

// ---------------------------------------------------------------------------
// Fork-site detection
// ---------------------------------------------------------------------------

/// Parameters of the function currently being scanned.
struct FnCtx<'a> {
    fn_name: &'a str,
    param_names: Vec<String>,
    /// `param - k` forms are only meaningful for int-like parameters.
    param_int_like: Vec<bool>,
}

impl FnCtx<'_> {
    fn param_index(&self, name: &str) -> Option<usize> {
        self.param_names.iter().position(|p| p == name)
    }
}

fn collect_sites(
    ast: &Output<'_>,
    recursive_pure: &RecursivePureSet,
    out: &mut HashMap<String, ParForkSite>,
) {
    match ast.1.as_ref() {
        Expression::Program(items) | Expression::Block(items) | Expression::Fragment(items) => {
            for item in items {
                collect_sites(item, recursive_pure, out);
            }
        }
        Expression::Module(_, body) => collect_sites(body, recursive_pure, out),
        Expression::Statement(inner)
        | Expression::Expr(inner)
        | Expression::ExprStatement(inner)
        | Expression::Group(inner) => collect_sites(inner, recursive_pure, out),
        Expression::Function {
            name,
            args,
            body: Some(body),
            ..
        } if recursive_pure.contains(*name) => {
            if let Some(site) = detect_fork_site(name, args, body) {
                out.insert((*name).to_string(), site);
            }
            collect_sites(body, recursive_pure, out);
        }
        Expression::Function {
            body: Some(body), ..
        } => collect_sites(body, recursive_pure, out),
        Expression::Implementation { methods, .. } => {
            for m in methods {
                collect_sites(m, recursive_pure, out);
            }
        }
        Expression::Method(_, inner) | Expression::Member(inner) => {
            collect_sites(inner, recursive_pure, out);
        }
        _ => {}
    }
}

fn detect_fork_site(name: &str, args: &Output<'_>, body: &Output<'_>) -> Option<ParForkSite> {
    let (param_names, param_int_like) = fn_params(args)?;
    let ctx = FnCtx {
        fn_name: name,
        param_names,
        param_int_like,
    };
    let mut found: Vec<(bool, ParForkSite)> = Vec::new();
    scan(body, &ctx, false, &mut found);
    let best = found
        .iter()
        .find(|(on_return, _)| *on_return)
        .or_else(|| found.first())?;
    Some(best.1.clone())
}

/// `(names, int_like)` for the declared parameters, in order.
fn fn_params(args: &Output<'_>) -> Option<(Vec<String>, Vec<bool>)> {
    let items = match args.1.as_ref() {
        Expression::Fragment(items) | Expression::Block(items) => items.as_slice(),
        _ => return None,
    };
    let mut names = Vec::new();
    let mut int_like = Vec::new();
    for item in items {
        let Expression::Argument { name, ty, .. } = peel(item).1.as_ref() else {
            continue;
        };
        let ty_name = ty.as_ref().and_then(|t| match peel(t).1.as_ref() {
            Expression::Type(n) | Expression::Identifier(n) => Some(*n),
            _ => None,
        });
        names.push((*name).to_string());
        int_like.push(matches!(ty_name, Some("int") | Some("byte")));
    }
    Some((names, int_like))
}

/// Depth-first body walk recording every fork site, tagged with whether it sits
/// on a return path (those win when picking the function's primary site).
fn scan(ast: &Output<'_>, ctx: &FnCtx<'_>, on_return: bool, found: &mut Vec<(bool, ParForkSite)>) {
    if let Some(site) = match_fork(ast, ctx) {
        found.push((on_return, site));
    }
    match ast.1.as_ref() {
        Expression::Program(items)
        | Expression::Block(items)
        | Expression::Fragment(items)
        | Expression::List(items)
        | Expression::Array(items)
        | Expression::Tuple(items)
        | Expression::If(items) => {
            for item in items {
                scan(item, ctx, on_return, found);
            }
        }
        Expression::Return(inner) | Expression::ImplicitReturn(inner) => {
            scan(inner, ctx, true, found);
        }
        Expression::Statement(inner)
        | Expression::Expr(inner)
        | Expression::ExprStatement(inner)
        | Expression::Group(inner)
        | Expression::Negate(inner)
        | Expression::Positive(inner)
        | Expression::Not(inner)
        | Expression::LogicalNot(inner)
        | Expression::Cast(inner, _)
        | Expression::Try(inner)
        | Expression::Readonly(inner) => scan(inner, ctx, on_return, found),
        Expression::Add(a, b)
        | Expression::Sub(a, b)
        | Expression::Mul(a, b)
        | Expression::Div(a, b)
        | Expression::Mod(a, b)
        | Expression::Eq(a, b)
        | Expression::Neq(a, b)
        | Expression::Le(a, b)
        | Expression::Gt(a, b)
        | Expression::Leq(a, b)
        | Expression::Geq(a, b)
        | Expression::Coalesce(a, b)
        | Expression::Assignment(a, b) => {
            scan(a, ctx, on_return, found);
            scan(b, ctx, on_return, found);
        }
        Expression::Call { name, args } => {
            scan(name, ctx, on_return, found);
            for a in args.iter().flatten() {
                scan(a, ctx, on_return, found);
            }
        }
        Expression::Construct { fields, .. } => match fields {
            EnumConstructPayload::Tuple(items) => {
                for item in items {
                    scan(item, ctx, on_return, found);
                }
            }
            EnumConstructPayload::Record(fields) => {
                for f in fields {
                    scan(&f.value, ctx, on_return, found);
                }
            }
            EnumConstructPayload::Unit => {}
        },
        Expression::Branch(cond, body) => {
            if let Some(c) = cond {
                scan(c, ctx, on_return, found);
            }
            scan(body, ctx, on_return, found);
        }
        // Arms are scanned independently — a fork never spans two arms.
        Expression::Match { scrutinee, arms } => {
            scan(scrutinee, ctx, on_return, found);
            for arm in arms {
                scan(&arm.body, ctx, on_return, found);
            }
        }
        Expression::For {
            init,
            cond,
            step,
            body,
        } => {
            if let Some(i) = init {
                scan(i, ctx, on_return, found);
            }
            scan(cond, ctx, on_return, found);
            if let Some(s) = step {
                scan(s, ctx, on_return, found);
            }
            scan(body, ctx, on_return, found);
        }
        Expression::Loop { iterable, body, .. } => {
            scan(iterable, ctx, on_return, found);
            scan(body, ctx, on_return, found);
        }
        Expression::Variable(_, Some(init)) | Expression::Constant(_, Some(init)) => {
            scan(init, ctx, on_return, found)
        }
        Expression::LetDestructure { rhs, .. } => scan(rhs, ctx, on_return, found),
        _ => {}
    }
}

/// Recognize the three IPA fork shapes at `expr` (no recursion).
fn match_fork(expr: &Output<'_>, ctx: &FnCtx<'_>) -> Option<ParForkSite> {
    let expr = peel(expr);
    match expr.1.as_ref() {
        Expression::Add(a, b) => binop_site(ctx, a, b, ParBinOp::Add),
        Expression::Sub(a, b) => binop_site(ctx, a, b, ParBinOp::Sub),
        Expression::Mul(a, b) => binop_site(ctx, a, b, ParBinOp::Mul),
        Expression::Construct {
            enum_name,
            variant_name,
            fields: EnumConstructPayload::Tuple(items),
        } => {
            let arms = self_call_arms(items, ctx)?;
            site(
                ctx,
                arms,
                ParCombine::EnumCtor {
                    enum_name: (*enum_name).to_string(),
                    variant_name: (*variant_name).to_string(),
                },
            )
        }
        Expression::Call {
            name,
            args: Some(args),
        } if callee_name(name) == Some(ctx.fn_name) => {
            let arms = self_call_arms(args, ctx)?;
            site(ctx, arms, ParCombine::SelfCall)
        }
        _ => None,
    }
}

fn binop_site(
    ctx: &FnCtx<'_>,
    a: &Output<'_>,
    b: &Output<'_>,
    op: ParBinOp,
) -> Option<ParForkSite> {
    let arms = vec![self_call_arm(a, ctx)?, self_call_arm(b, ctx)?];
    site(ctx, arms, ParCombine::BinOp(op))
}

fn site(ctx: &FnCtx<'_>, arms: Vec<ParArm>, combine: ParCombine) -> Option<ParForkSite> {
    if arms.len() < 2 {
        return None;
    }
    Some(ParForkSite {
        fn_name: ctx.fn_name.to_string(),
        param_count: ctx.param_names.len(),
        arms,
        combine,
    })
}

/// Every operand must be an independent self-call: the combine consumes arm
/// results positionally, so a mixed operand list is not representable.
fn self_call_arms(items: &[Output<'_>], ctx: &FnCtx<'_>) -> Option<Vec<ParArm>> {
    if items.len() < 2 {
        return None;
    }
    items.iter().map(|i| self_call_arm(i, ctx)).collect()
}

fn self_call_arm(expr: &Output<'_>, ctx: &FnCtx<'_>) -> Option<ParArm> {
    let expr = peel(expr);
    let Expression::Call {
        name,
        args: Some(args),
    } = expr.1.as_ref()
    else {
        return None;
    };
    if callee_name(name) != Some(ctx.fn_name) || args.len() != ctx.param_names.len() {
        return None;
    }
    let forms = args
        .iter()
        .map(|a| arg_form(a, ctx))
        .collect::<Option<Vec<_>>>()?;
    Some(ParArm::SelfCall { args: forms })
}

fn arg_form(expr: &Output<'_>, ctx: &FnCtx<'_>) -> Option<ArgForm> {
    let expr = peel(expr);
    match expr.1.as_ref() {
        Expression::Integer(k) => Some(ArgForm::Const(*k)),
        Expression::Identifier(p) => ctx.param_index(p).map(ArgForm::Param),
        Expression::Sub(lhs, rhs) => {
            let (Expression::Identifier(p), Expression::Integer(k)) =
                (peel(lhs).1.as_ref(), peel(rhs).1.as_ref())
            else {
                return None;
            };
            if *k <= 0 {
                return None;
            }
            let idx = ctx.param_index(p)?;
            ctx.param_int_like
                .get(idx)
                .copied()
                .unwrap_or(false)
                .then_some(ArgForm::ParamMinus {
                    param: idx,
                    sub: *k,
                })
        }
        _ => None,
    }
}

fn callee_name<'a>(name: &'a Output<'a>) -> Option<&'a str> {
    match peel(name).1.as_ref() {
        Expression::Identifier(n) => Some(*n),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Constant call-site collection
// ---------------------------------------------------------------------------

fn collect_const_calls(
    ast: &Output<'_>,
    sites: &HashMap<String, ParForkSite>,
    out: &mut HashMap<String, BTreeSet<Vec<i64>>>,
) {
    match ast.1.as_ref() {
        Expression::Program(items)
        | Expression::Block(items)
        | Expression::Fragment(items)
        | Expression::List(items)
        | Expression::Array(items)
        | Expression::Tuple(items)
        | Expression::If(items) => {
            for item in items {
                collect_const_calls(item, sites, out);
            }
        }
        Expression::Module(_, body)
        | Expression::Statement(body)
        | Expression::Expr(body)
        | Expression::ExprStatement(body)
        | Expression::Group(body)
        | Expression::Return(body)
        | Expression::ImplicitReturn(body)
        | Expression::Negate(body)
        | Expression::Not(body)
        | Expression::LogicalNot(body)
        | Expression::Positive(body)
        | Expression::Cast(body, _)
        | Expression::Try(body)
        | Expression::Readonly(body) => collect_const_calls(body, sites, out),
        Expression::Add(a, b)
        | Expression::Sub(a, b)
        | Expression::Mul(a, b)
        | Expression::Div(a, b)
        | Expression::Mod(a, b)
        | Expression::Assignment(a, b)
        | Expression::Eq(a, b)
        | Expression::Neq(a, b)
        | Expression::Le(a, b)
        | Expression::Gt(a, b)
        | Expression::Leq(a, b)
        | Expression::Geq(a, b)
        | Expression::Coalesce(a, b) => {
            collect_const_calls(a, sites, out);
            collect_const_calls(b, sites, out);
        }
        Expression::Call { name, args } => {
            collect_const_calls(name, sites, out);
            let Some(args) = args else {
                return;
            };
            for a in args {
                collect_const_calls(a, sites, out);
            }
            let Some(fname) = callee_name(name) else {
                return;
            };
            let Some(site) = sites.get(fname) else {
                return;
            };
            if args.len() != site.param_count {
                return;
            }
            let consts = args
                .iter()
                .map(|a| match peel(a).1.as_ref() {
                    Expression::Integer(n) => Some(*n),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>();
            if let Some(consts) = consts {
                if const_args_worth_parallel(&consts) {
                    out.entry(fname.to_string()).or_default().insert(consts);
                }
            }
        }
        Expression::Construct { fields, .. } => match fields {
            EnumConstructPayload::Tuple(items) => {
                for item in items {
                    collect_const_calls(item, sites, out);
                }
            }
            EnumConstructPayload::Record(fields) => {
                for f in fields {
                    collect_const_calls(&f.value, sites, out);
                }
            }
            EnumConstructPayload::Unit => {}
        },
        Expression::Branch(cond, body) => {
            if let Some(c) = cond {
                collect_const_calls(c, sites, out);
            }
            collect_const_calls(body, sites, out);
        }
        Expression::Match { scrutinee, arms } => {
            collect_const_calls(scrutinee, sites, out);
            for arm in arms {
                collect_const_calls(&arm.body, sites, out);
            }
        }
        Expression::For {
            init,
            cond,
            step,
            body,
        } => {
            if let Some(i) = init {
                collect_const_calls(i, sites, out);
            }
            collect_const_calls(cond, sites, out);
            if let Some(s) = step {
                collect_const_calls(s, sites, out);
            }
            collect_const_calls(body, sites, out);
        }
        Expression::Loop {
            identifier,
            iterable,
            body,
        } => {
            if let Some(id) = identifier {
                collect_const_calls(id, sites, out);
            }
            collect_const_calls(iterable, sites, out);
            collect_const_calls(body, sites, out);
        }
        Expression::Variable(_, Some(init)) | Expression::Constant(_, Some(init)) => {
            collect_const_calls(init, sites, out);
        }
        Expression::LetDestructure { rhs, .. } => collect_const_calls(rhs, sites, out),
        Expression::Function {
            body: Some(body), ..
        }
        | Expression::Lambda { body, .. }
        | Expression::Defer { body, .. } => collect_const_calls(body, sites, out),
        Expression::Implementation { methods, .. } => {
            for m in methods {
                collect_const_calls(m, sites, out);
            }
        }
        Expression::Method(_, inner) | Expression::Member(inner) => {
            collect_const_calls(inner, sites, out);
        }
        _ => {}
    }
}

fn peel<'a>(expr: &'a Output<'a>) -> &'a Output<'a> {
    match expr.1.as_ref() {
        Expression::Expr(inner)
        | Expression::Group(inner)
        | Expression::Statement(inner)
        | Expression::ExprStatement(inner) => peel(inner),
        Expression::Fragment(items) if items.len() == 1 => peel(&items[0]),
        _ => expr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typechecking::purity::analyze_recursive_pure;
    use parser::Pratt;

    fn parse(src: &str) -> Output<'static> {
        // Leak for test simplicity — parse owns string via 'static trick.
        let owned = Box::leak(src.to_string().into_boxed_str());
        Pratt::default().parse(owned).expect("parse")
    }

    fn sites_of(src: &str) -> HashMap<String, ParForkSite> {
        let ast = parse(src);
        let pure = analyze_recursive_pure(&ast);
        analyze_par_fork_sites(&ast, &pure)
    }

    fn arm_args(site: &ParForkSite, i: usize) -> &[ArgForm] {
        let ParArm::SelfCall { args } = &site.arms[i];
        args
    }

    #[test]
    fn detects_fib_shape_and_const_calls() {
        let ast = parse(
            r#"
fn fib(int n) -> int {
    if n <= 2 { return 1; }
    return fib(n - 1) + fib(n - 2);
}
fn main() {
    let x = fib(32);
    return;
}
"#,
        );
        let pure = analyze_recursive_pure(&ast);
        assert!(pure.contains("fib"));
        let sites = analyze_par_fork_sites(&ast, &pure);
        let fib = sites.get("fib").expect("fib fork site");
        assert_eq!(fib.param_count, 1);
        assert_eq!(fib.combine, ParCombine::BinOp(ParBinOp::Add));
        assert_eq!(arm_args(fib, 0), [ArgForm::ParamMinus { param: 0, sub: 1 }]);
        assert_eq!(arm_args(fib, 1), [ArgForm::ParamMinus { param: 0, sub: 2 }]);

        let demanded = collect_par_specialization_args(&ast, &sites);
        let set = demanded.get("fib").expect("fib demands");
        assert!(set.contains(&vec![32]));
        assert!(set.contains(&vec![21])); // chain toward threshold
        assert!(!set.contains(&vec![20]));
    }

    #[test]
    fn detects_mul_binop_fork() {
        let sites = sites_of(
            r#"
fn tree(int n) -> int {
    if n <= 1 { return 1; }
    return tree(n - 1) * tree(n - 2);
}
fn main() { return; }
"#,
        );
        let tree = sites.get("tree").expect("tree fork site");
        assert_eq!(tree.combine, ParCombine::BinOp(ParBinOp::Mul));
        assert_eq!(
            arm_args(tree, 0),
            [ArgForm::ParamMinus { param: 0, sub: 1 }]
        );
        assert_eq!(
            arm_args(tree, 1),
            [ArgForm::ParamMinus { param: 0, sub: 2 }]
        );
    }

    #[test]
    fn rejects_single_recursive_arm() {
        let sites = sites_of(
            r#"
fn fib(int n) -> int {
    if n <= 1 { return n; }
    return fib(n - 1) + (n - 2);
}
fn main() { return; }
"#,
        );
        assert!(
            !sites.contains_key("fib"),
            "single recursive arm must not be a fork site: {sites:?}"
        );
    }

    #[test]
    fn detects_enum_ctor_fork() {
        let sites = sites_of(
            r#"
enum Tree {
    Leaf,
    Node(Tree, Tree),
}
fn build(int n) -> Tree {
    if n <= 1 { return Tree::Leaf; }
    return Tree::Node(build(n - 1), build(n - 2));
}
fn main() { return; }
"#,
        );
        let build = sites.get("build").expect("build fork site");
        assert_eq!(
            build.combine,
            ParCombine::EnumCtor {
                enum_name: "Tree".to_string(),
                variant_name: "Node".to_string(),
            }
        );
        assert_eq!(build.arms.len(), 2);
        assert_eq!(
            arm_args(build, 0),
            [ArgForm::ParamMinus { param: 0, sub: 1 }]
        );
    }

    #[test]
    fn detects_tak_self_call_combine() {
        let sites = sites_of(
            r#"
fn tak(int x, int y, int z) -> int {
    if y < x {
        return tak(tak(x - 1, y, z), tak(y - 1, z, x), tak(z - 1, x, y));
    }
    return z;
}
fn main() { return; }
"#,
        );
        let tak = sites.get("tak").expect("tak fork site");
        assert_eq!(tak.combine, ParCombine::SelfCall);
        assert_eq!(tak.param_count, 3);
        assert_eq!(tak.arms.len(), 3);
        assert_eq!(
            arm_args(tak, 0),
            [
                ArgForm::ParamMinus { param: 0, sub: 1 },
                ArgForm::Param(1),
                ArgForm::Param(2)
            ]
        );
        assert_eq!(
            arm_args(tak, 2),
            [
                ArgForm::ParamMinus { param: 2, sub: 1 },
                ArgForm::Param(0),
                ArgForm::Param(1)
            ]
        );
        assert_eq!(
            eval_arm_args(&tak.arms[1], &[18, 12, 6]),
            Some(vec![11, 6, 18])
        );
    }

    #[test]
    fn detects_fork_inside_match_arm() {
        let sites = sites_of(
            r#"
enum Mode {
    Fast,
    Slow,
}
fn pick(int n) -> Mode {
    if n <= 1 { return Mode::Fast; }
    return Mode::Slow;
}
fn fibm(int n) -> int {
    return match pick(n) {
        Mode::Fast => 1,
        Mode::Slow => fibm(n - 1) + fibm(n - 2),
    };
}
fn main() { return; }
"#,
        );
        let fibm = sites.get("fibm").expect("fibm fork site");
        assert_eq!(fibm.combine, ParCombine::BinOp(ParBinOp::Add));
        assert_eq!(
            arm_args(fibm, 0),
            [ArgForm::ParamMinus { param: 0, sub: 1 }]
        );
        assert_eq!(
            arm_args(fibm, 1),
            [ArgForm::ParamMinus { param: 0, sub: 2 }]
        );
    }

    #[test]
    fn below_threshold_and_dynamic_args_do_not_demand_specs() {
        let t = par_cost_threshold();
        let ast = parse(&format!(
            r#"
fn fib(int n) -> int {{
    if n <= 1 {{ return n; }}
    return fib(n - 1) + fib(n - 2);
}}
fn main() {{
    let k = {t};
    let a = fib({t});
    let b = fib(k);
    return;
}}
"#
        ));
        let pure = analyze_recursive_pure(&ast);
        let sites = analyze_par_fork_sites(&ast, &pure);
        let demanded = collect_par_specialization_args(&ast, &sites);
        assert!(
            demanded.get("fib").is_none(),
            "arg == threshold and dynamic args must not demand specs: {demanded:?}"
        );
        assert!(!const_args_worth_parallel(&[t]));
        assert!(const_args_worth_parallel(&[t + 1]));
        assert!(!const_args_worth_parallel(&[]));
    }

    #[test]
    fn specialization_names_cover_multi_arg() {
        assert_eq!(par_specialization_name("fib", &[22]), "__coil_par_fib_22");
        assert_eq!(
            par_specialization_name("tak", &[18, 12, 6]),
            "__coil_par_tak_18_12_6"
        );
    }
}
