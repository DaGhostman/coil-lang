//! Static profitability for auto-parallel recursion (no runtime threshold checks).
//!
//! Detects unary recursive-pure shapes `f(n-a) ⊕ f(n-b)` and decides whether a
//! concrete int argument is worth forking. Call sites with constant args above
//! [`par_int_threshold`] are rewritten to specialized nullary clones that always
//! fork (fully static).

use std::collections::{BTreeSet, HashMap, HashSet};

use parser::ast::{Expression, Output};

use super::purity::RecursivePureSet;

/// Compile-time fork threshold (`COIL_PAR_THRESHOLD`, default 20).
pub fn par_int_threshold() -> i64 {
    static T: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *T.get_or_init(|| {
        std::env::var("COIL_PAR_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(20)
    })
}

/// Binary op used at the recursive fork site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParBinOp {
    Add,
    Sub,
    Mul,
}

/// Unary recursive-pure function with `f(n - left) ⊕ f(n - right)` shape.
#[derive(Debug, Clone)]
pub struct RecParShape {
    pub fn_name: String,
    #[allow(dead_code)]
    pub param: String,
    /// `f(n - left_sub)`.
    pub left_sub: i64,
    /// `f(n - right_sub)`.
    pub right_sub: i64,
    pub op: ParBinOp,
    /// From `if n <= base_le` / `n < base_lt` when present (informational).
    #[allow(dead_code)]
    pub base_bound: Option<i64>,
}

/// True when a known int argument should use the parallel clone.
pub fn const_arg_worth_parallel(arg: i64) -> bool {
    arg > par_int_threshold()
}

/// Specialized nullary entry name for `fn_name` at concrete `n`.
pub fn par_specialization_name(fn_name: &str, n: i64) -> String {
    format!("__coil_par_{fn_name}_{n}")
}

/// Collect shapes for recursive-pure functions that match the fork pattern.
pub fn analyze_rec_par_shapes(
    ast: &Output<'_>,
    recursive_pure: &RecursivePureSet,
) -> HashMap<String, RecParShape> {
    let mut out = HashMap::new();
    collect_shapes(ast, recursive_pure, &mut out);
    out
}

/// Constant call-site arguments to recursive-pure functions (and the full
/// specialization chains they require).
pub fn collect_par_specialization_args(
    ast: &Output<'_>,
    shapes: &HashMap<String, RecParShape>,
) -> HashMap<String, BTreeSet<i64>> {
    let mut demanded: HashMap<String, BTreeSet<i64>> = HashMap::new();
    collect_const_calls(ast, shapes, &mut demanded);
    let t = par_int_threshold();
    // Close under left/right subtractions down to threshold+1.
    let names: Vec<String> = demanded.keys().cloned().collect();
    for name in names {
        let Some(shape) = shapes.get(&name) else {
            continue;
        };
        let mut set = demanded.remove(&name).unwrap_or_default();
        let mut stack: Vec<i64> = set.iter().copied().collect();
        let mut seen = HashSet::new();
        while let Some(n) = stack.pop() {
            if n <= t || !seen.insert(n) {
                continue;
            }
            set.insert(n);
            let left = n - shape.left_sub;
            let right = n - shape.right_sub;
            if left > t {
                stack.push(left);
            }
            if right > t {
                stack.push(right);
            }
        }
        set.retain(|n| *n > t);
        if !set.is_empty() {
            demanded.insert(name, set);
        }
    }
    demanded
}

fn collect_shapes(
    ast: &Output<'_>,
    recursive_pure: &RecursivePureSet,
    out: &mut HashMap<String, RecParShape>,
) {
    match ast.1.as_ref() {
        Expression::Program(items) | Expression::Block(items) | Expression::Fragment(items) => {
            for item in items {
                collect_shapes(item, recursive_pure, out);
            }
        }
        Expression::Module(_, body) => collect_shapes(body, recursive_pure, out),
        Expression::Statement(inner)
        | Expression::Expr(inner)
        | Expression::ExprStatement(inner)
        | Expression::Group(inner) => collect_shapes(inner, recursive_pure, out),
        Expression::Function {
            name,
            args,
            body: Some(body),
            ..
        } if recursive_pure.contains(*name) => {
            if let Some(shape) = detect_shape(name, args, body) {
                out.insert((*name).to_string(), shape);
            }
            collect_shapes(body, recursive_pure, out);
        }
        Expression::Function {
            body: Some(body), ..
        } => collect_shapes(body, recursive_pure, out),
        Expression::Implementation { methods, .. } => {
            for m in methods {
                collect_shapes(m, recursive_pure, out);
            }
        }
        Expression::Method(_, inner) | Expression::Member(inner) => {
            collect_shapes(inner, recursive_pure, out);
        }
        _ => {}
    }
}

fn detect_shape(name: &str, args: &Output<'_>, body: &Output<'_>) -> Option<RecParShape> {
    let param = single_int_param_name(args)?;
    let (base_bound, fork) = find_base_and_fork(body, name, &param)?;
    let (left_sub, right_sub, op) = fork;
    if left_sub <= 0 || right_sub <= 0 {
        return None;
    }
    Some(RecParShape {
        fn_name: name.to_string(),
        param,
        left_sub,
        right_sub,
        op,
        base_bound,
    })
}

fn single_int_param_name(args: &Output<'_>) -> Option<String> {
    let items = match args.1.as_ref() {
        Expression::Fragment(items) | Expression::Block(items) => items.as_slice(),
        _ => return None,
    };
    let mut found = None;
    for item in items {
        let arg = peel(item);
        let Expression::Argument { name, ty, .. } = arg.1.as_ref() else {
            continue;
        };
        let Some(ty) = ty else {
            return None;
        };
        let ty_name = match peel(ty).1.as_ref() {
            Expression::Type(t) | Expression::Identifier(t) => *t,
            _ => continue,
        };
        if !matches!(ty_name, "int" | "byte") {
            return None;
        }
        if found.is_some() {
            return None; // must be unary
        }
        found = Some((*name).to_string());
    }
    found
}

/// Walk body for optional `if n <= K` / `n < K` and a recursive binop return.
fn find_base_and_fork(
    body: &Output<'_>,
    fn_name: &str,
    param: &str,
) -> Option<(Option<i64>, (i64, i64, ParBinOp))> {
    let mut base = None;
    let mut fork = None;
    walk_for_shape(body, fn_name, param, &mut base, &mut fork);
    fork.map(|f| (base, f))
}

fn walk_for_shape(
    ast: &Output<'_>,
    fn_name: &str,
    param: &str,
    base: &mut Option<i64>,
    fork: &mut Option<(i64, i64, ParBinOp)>,
) {
    match ast.1.as_ref() {
        Expression::Program(items)
        | Expression::Block(items)
        | Expression::Fragment(items)
        | Expression::If(items) => {
            for item in items {
                walk_for_shape(item, fn_name, param, base, fork);
            }
        }
        Expression::Branch(cond, body) => {
            if let Some(c) = cond {
                if let Some(b) = match_base_bound(c, param) {
                    *base = Some(b);
                }
            }
            walk_for_shape(body, fn_name, param, base, fork);
        }
        Expression::Statement(inner)
        | Expression::Expr(inner)
        | Expression::ExprStatement(inner)
        | Expression::Group(inner)
        | Expression::Return(inner)
        | Expression::ImplicitReturn(inner) => {
            if let Some(f) = match_rec_binop(inner, fn_name, param) {
                *fork = Some(f);
            }
            walk_for_shape(inner, fn_name, param, base, fork);
        }
        Expression::Add(a, b) => {
            if let Some(f) = match_rec_binop(ast, fn_name, param) {
                *fork = Some(f);
            }
            walk_for_shape(a, fn_name, param, base, fork);
            walk_for_shape(b, fn_name, param, base, fork);
        }
        Expression::Sub(a, b) | Expression::Mul(a, b) => {
            if let Some(f) = match_rec_binop(ast, fn_name, param) {
                *fork = Some(f);
            }
            walk_for_shape(a, fn_name, param, base, fork);
            walk_for_shape(b, fn_name, param, base, fork);
        }
        _ => {}
    }
}

fn match_base_bound(cond: &Output<'_>, param: &str) -> Option<i64> {
    let cond = peel(cond);
    match cond.1.as_ref() {
        Expression::Leq(lhs, rhs) | Expression::Le(lhs, rhs) => {
            let lhs = peel(lhs);
            let rhs = peel(rhs);
            let is_le = matches!(cond.1.as_ref(), Expression::Leq(_, _));
            match (lhs.1.as_ref(), rhs.1.as_ref()) {
                (Expression::Identifier(p), Expression::Integer(k)) if *p == param => {
                    // n <= k → bound k; n < k → bound k-1 for "last base"
                    Some(if is_le { *k } else { *k - 1 })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn match_rec_binop(expr: &Output<'_>, fn_name: &str, param: &str) -> Option<(i64, i64, ParBinOp)> {
    let expr = peel(expr);
    let (a, b, op) = match expr.1.as_ref() {
        Expression::Add(a, b) => (a, b, ParBinOp::Add),
        Expression::Sub(a, b) => (a, b, ParBinOp::Sub),
        Expression::Mul(a, b) => (a, b, ParBinOp::Mul),
        _ => return None,
    };
    let left = match_rec_call_sub(a, fn_name, param)?;
    let right = match_rec_call_sub(b, fn_name, param)?;
    Some((left, right, op))
}

fn match_rec_call_sub(expr: &Output<'_>, fn_name: &str, param: &str) -> Option<i64> {
    let expr = peel(expr);
    let Expression::Call {
        name,
        args: Some(args),
    } = expr.1.as_ref()
    else {
        return None;
    };
    if args.len() != 1 {
        return None;
    }
    let callee = match peel(name).1.as_ref() {
        Expression::Identifier(n) => *n,
        _ => return None,
    };
    if callee != fn_name {
        return None;
    }
    match_param_minus_const(peel(&args[0]), param)
}

fn match_param_minus_const(expr: &Output<'_>, param: &str) -> Option<i64> {
    let expr = peel(expr);
    match expr.1.as_ref() {
        Expression::Sub(lhs, rhs) => {
            let lhs = peel(lhs);
            let rhs = peel(rhs);
            match (lhs.1.as_ref(), rhs.1.as_ref()) {
                (Expression::Identifier(p), Expression::Integer(k)) if *p == param && *k > 0 => {
                    Some(*k)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn collect_const_calls(
    ast: &Output<'_>,
    shapes: &HashMap<String, RecParShape>,
    out: &mut HashMap<String, BTreeSet<i64>>,
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
                collect_const_calls(item, shapes, out);
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
        | Expression::Readonly(body) => collect_const_calls(body, shapes, out),
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
            collect_const_calls(a, shapes, out);
            collect_const_calls(b, shapes, out);
        }
        Expression::Call { name, args } => {
            collect_const_calls(name, shapes, out);
            if let Some(args) = args {
                for a in args {
                    collect_const_calls(a, shapes, out);
                }
                if args.len() == 1 {
                    if let Expression::Identifier(fname) = peel(name).1.as_ref() {
                        if shapes.contains_key(*fname) {
                            if let Expression::Integer(n) = peel(&args[0]).1.as_ref() {
                                if const_arg_worth_parallel(*n) {
                                    out.entry((*fname).to_string())
                                        .or_default()
                                        .insert(*n);
                                }
                            }
                        }
                    }
                }
            }
        }
        Expression::Branch(cond, body) => {
            if let Some(c) = cond {
                collect_const_calls(c, shapes, out);
            }
            collect_const_calls(body, shapes, out);
        }
        Expression::Match { scrutinee, arms } => {
            collect_const_calls(scrutinee, shapes, out);
            for arm in arms {
                collect_const_calls(&arm.body, shapes, out);
            }
        }
        Expression::For {
            init,
            cond,
            step,
            body,
        } => {
            if let Some(i) = init {
                collect_const_calls(i, shapes, out);
            }
            collect_const_calls(cond, shapes, out);
            if let Some(s) = step {
                collect_const_calls(s, shapes, out);
            }
            collect_const_calls(body, shapes, out);
        }
        Expression::Loop {
            identifier,
            iterable,
            body,
        } => {
            if let Some(id) = identifier {
                collect_const_calls(id, shapes, out);
            }
            collect_const_calls(iterable, shapes, out);
            collect_const_calls(body, shapes, out);
        }
        Expression::Variable(_, Some(init)) | Expression::Constant(_, Some(init)) => {
            collect_const_calls(init, shapes, out);
        }
        Expression::Function {
            body: Some(body), ..
        }
        | Expression::Lambda { body, .. }
        | Expression::Defer { body, .. } => collect_const_calls(body, shapes, out),
        Expression::Implementation { methods, .. } => {
            for m in methods {
                collect_const_calls(m, shapes, out);
            }
        }
        Expression::Method(_, inner) | Expression::Member(inner) => {
            collect_const_calls(inner, shapes, out);
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
        let shapes = analyze_rec_par_shapes(&ast, &pure);
        let fib = shapes.get("fib").expect("fib shape");
        assert_eq!(fib.left_sub, 1);
        assert_eq!(fib.right_sub, 2);
        assert_eq!(fib.op, ParBinOp::Add);
        assert_eq!(fib.param, "n");
        assert_eq!(fib.base_bound, Some(2));
        let demanded = collect_par_specialization_args(&ast, &shapes);
        let set = demanded.get("fib").expect("fib demands");
        assert!(set.contains(&32));
        assert!(set.contains(&21)); // chain toward threshold
        assert!(!set.contains(&20));
    }

    #[test]
    fn detects_mul_rec_par_shape() {
        let ast = parse(
            r#"
fn tree(int n) -> int {
    if n <= 1 { return 1; }
    return tree(n - 1) * tree(n - 2);
}
fn main() { return; }
"#,
        );
        let pure = analyze_recursive_pure(&ast);
        let shapes = analyze_rec_par_shapes(&ast, &pure);
        let tree = shapes.get("tree").expect("tree shape");
        assert_eq!(tree.op, ParBinOp::Mul);
        assert_eq!(tree.left_sub, 1);
        assert_eq!(tree.right_sub, 2);
    }

    #[test]
    fn rejects_non_dual_recursive_binop() {
        let ast = parse(
            r#"
fn fib(int n) -> int {
    if n <= 1 { return n; }
    return fib(n - 1) + (n - 2);
}
fn main() { return; }
"#,
        );
        let pure = analyze_recursive_pure(&ast);
        assert!(pure.contains("fib"));
        let shapes = analyze_rec_par_shapes(&ast, &pure);
        assert!(
            !shapes.contains_key("fib"),
            "single recursive arm must not be a par shape: {shapes:?}"
        );
    }

    #[test]
    fn below_threshold_and_dynamic_args_do_not_demand_specs() {
        let t = par_int_threshold();
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
        let shapes = analyze_rec_par_shapes(&ast, &pure);
        let demanded = collect_par_specialization_args(&ast, &shapes);
        assert!(
            demanded.get("fib").is_none(),
            "arg == threshold and dynamic args must not demand specs: {demanded:?}"
        );
        assert!(!const_arg_worth_parallel(t));
        assert!(const_arg_worth_parallel(t + 1));
        assert_eq!(par_specialization_name("fib", t + 1), format!("__coil_par_fib_{}", t + 1));
    }
}
