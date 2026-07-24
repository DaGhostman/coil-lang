//! Compile-time scalar constant evaluation for codegen optimizations.

use std::collections::HashMap;

use parser::{
    SimpleSpan,
    ast::{Expression, Output},
};

/// A scalar value known at compile time.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

impl ConstValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConstValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

/// Evaluate a pure expression using `env` for const identifiers.
pub fn eval_expr<'a>(
    ast: &(SimpleSpan, Box<Expression<'a>>),
    env: &HashMap<String, ConstValue>,
) -> Option<ConstValue> {
    match ast.1.as_ref() {
        Expression::Integer(n) => Some(ConstValue::Int(*n)),
        Expression::Float(n) => Some(ConstValue::Float(*n)),
        Expression::Bool(b) => Some(ConstValue::Bool(*b)),
        Expression::String(s) => Some(ConstValue::Str(
            s.replace("\\n", "\n")
                .replace("\\r", "\r")
                .replace("\\t", "\t")
                .replace("\\0", "\0"),
        )),
        Expression::Identifier(name) => env.get(*name).cloned(),
        Expression::Group(inner) | Expression::Expr(inner) | Expression::Statement(inner) => {
            eval_expr(inner, env)
        }
        Expression::Positive(inner) => eval_expr(inner, env),
        Expression::Negate(inner) => {
            let v = eval_expr(inner, env)?;
            match v {
                ConstValue::Int(n) => Some(ConstValue::Int(-n)),
                ConstValue::Float(n) => Some(ConstValue::Float(-n)),
                _ => None,
            }
        }
        Expression::Not(inner) | Expression::LogicalNot(inner) => {
            let v = eval_expr(inner, env)?;
            match v {
                ConstValue::Int(n) => Some(ConstValue::Bool(n == 0)),
                ConstValue::Bool(b) => Some(ConstValue::Bool(!b)),
                _ => None,
            }
        }
        Expression::Add(lhs, rhs) => eval_string_add(lhs, rhs, env).or_else(|| {
            eval_binop(lhs, rhs, env, |a, b| a + b, |a, b| a + b)
        }),
        Expression::Sub(lhs, rhs) => eval_binop(lhs, rhs, env, |a, b| a - b, |a, b| a - b),
        Expression::Mul(lhs, rhs) => eval_binop(lhs, rhs, env, |a, b| a * b, |a, b| a * b),
        Expression::Div(lhs, rhs) => {
            let a = eval_expr(lhs, env)?;
            let b = eval_expr(rhs, env)?;
            match (a, b) {
                (ConstValue::Int(x), ConstValue::Int(y)) if y != 0 => {
                    Some(ConstValue::Int(x / y))
                }
                (ConstValue::Float(x), ConstValue::Float(y)) if y != 0.0 && y.is_finite() => {
                    Some(ConstValue::Float(x / y))
                }
                _ => None,
            }
        }
        Expression::Mod(lhs, rhs) => {
            let a = eval_expr(lhs, env)?;
            let b = eval_expr(rhs, env)?;
            match (a, b) {
                (ConstValue::Int(x), ConstValue::Int(y)) if y != 0 => {
                    Some(ConstValue::Int(x % y))
                }
                _ => None,
            }
        }
        Expression::Le(lhs, rhs) => eval_cmp(lhs, rhs, env, |a, b| a < b),
        Expression::Gt(lhs, rhs) => eval_cmp(lhs, rhs, env, |a, b| a > b),
        Expression::Leq(lhs, rhs) => eval_cmp(lhs, rhs, env, |a, b| a <= b),
        Expression::Geq(lhs, rhs) => eval_cmp(lhs, rhs, env, |a, b| a >= b),
        Expression::Eq(lhs, rhs) => eval_eq(lhs, rhs, env),
        Expression::Neq(lhs, rhs) => {
            eval_eq(lhs, rhs, env).map(|b| ConstValue::Bool(!matches!(b, ConstValue::Bool(true))))
        }
        _ => None,
    }
}

fn eval_binop<'a>(
    lhs: &Output<'a>,
    rhs: &Output<'a>,
    env: &HashMap<String, ConstValue>,
    int_op: fn(i64, i64) -> i64,
    float_op: fn(f64, f64) -> f64,
) -> Option<ConstValue> {
    let a = eval_expr(lhs, env)?;
    let b = eval_expr(rhs, env)?;
    match (a, b) {
        (ConstValue::Int(x), ConstValue::Int(y)) => Some(ConstValue::Int(int_op(x, y))),
        (ConstValue::Float(x), ConstValue::Float(y)) => Some(ConstValue::Float(float_op(x, y))),
        _ => None,
    }
}

fn eval_cmp<'a>(
    lhs: &Output<'a>,
    rhs: &Output<'a>,
    env: &HashMap<String, ConstValue>,
    cmp: fn(i64, i64) -> bool,
) -> Option<ConstValue> {
    let a = eval_expr(lhs, env)?;
    let b = eval_expr(rhs, env)?;
    match (a, b) {
        (ConstValue::Int(x), ConstValue::Int(y)) => Some(ConstValue::Bool(cmp(x, y))),
        _ => None,
    }
}

fn eval_eq<'a>(
    lhs: &Output<'a>,
    rhs: &Output<'a>,
    env: &HashMap<String, ConstValue>,
) -> Option<ConstValue> {
    let a = eval_expr(lhs, env)?;
    let b = eval_expr(rhs, env)?;
    Some(ConstValue::Bool(a == b))
}

/// String concatenation when both sides are known strings.
pub fn eval_string_add<'a>(
    lhs: &Output<'a>,
    rhs: &Output<'a>,
    env: &HashMap<String, ConstValue>,
) -> Option<ConstValue> {
    let a = eval_expr(lhs, env)?;
    let b = eval_expr(rhs, env)?;
    match (a, b) {
        (ConstValue::Str(x), ConstValue::Str(y)) => Some(ConstValue::Str(format!("{x}{y}"))),
        _ => None,
    }
}

/// Integer strength-reduction hint: `x * k` when k is 2 → shift left 1.
pub fn strength_mul_int(k: i64) -> Option<u32> {
    if k > 0 && (k & (k - 1)) == 0 {
        Some(k.trailing_zeros())
    } else {
        None
    }
}

/// If `expr` is `x + 0`, `x - 0`, `x * 1`, `x / 1`, `x % 1` (when defined), return inner.
pub fn strength_reduced_inner<'a>(
    expr: &'a (SimpleSpan, Box<Expression<'a>>),
) -> Option<&'a Output<'a>> {
    match expr.1.as_ref() {
        Expression::Add(lhs, rhs) => zero_int(rhs).map(|_| lhs).or_else(|| zero_int(lhs).map(|_| rhs)),
        Expression::Sub(lhs, rhs) if zero_int(rhs).is_some() => Some(lhs),
        Expression::Mul(lhs, rhs) => one_int(rhs).map(|_| lhs).or_else(|| one_int(lhs).map(|_| rhs)),
        Expression::Div(lhs, rhs) if one_int(rhs).is_some() => Some(lhs),
        _ => None,
    }
}

fn zero_int<'a>(expr: &Output<'a>) -> Option<()> {
    eval_expr(expr, &HashMap::new()).and_then(|v| match v {
        ConstValue::Int(0) => Some(()),
        _ => None,
    })
}

fn one_int<'a>(expr: &Output<'a>) -> Option<()> {
    eval_expr(expr, &HashMap::new()).and_then(|v| match v {
        ConstValue::Int(1) => Some(()),
        _ => None,
    })
}

/// C-style `for` trip count when init/cond/step match `i = 0; i < N; i = i + 1`.
pub fn for_loop_trip_count<'a>(
    init: Option<&Output<'a>>,
    cond: &Output<'a>,
    step: Option<&Output<'a>>,
) -> Option<u32> {
    let init = init?;
    let step = step?;
    if !for_init_sets_zero(init) {
        return None;
    }
    if !for_step_increments_by_one(step) {
        return None;
    }
    eval_for_upper_bound(cond)
}

fn eval_for_upper_bound<'a>(cond: &Output<'a>) -> Option<u32> {
    match cond.1.as_ref() {
        Expression::Le(lhs, rhs) => {
            let _i_name = ident_name(lhs)?;
            let bound = eval_expr(rhs, &HashMap::new())?;
            let ConstValue::Int(n) = bound else {
                return None;
            };
            if n < 0 || n > 8 {
                return None;
            }
            Some(n as u32)
        }
        Expression::Leq(lhs, rhs) => {
            let _i_name = ident_name(lhs)?;
            let bound = eval_expr(rhs, &HashMap::new())?;
            let ConstValue::Int(n) = bound else {
                return None;
            };
            if n < 0 || n > 7 {
                return None;
            }
            Some((n + 1) as u32)
        }
        _ => None,
    }
}

fn ident_name<'a>(expr: &Output<'a>) -> Option<&'a str> {
    match expr.1.as_ref() {
        Expression::Identifier(n) => Some(*n),
        _ => None,
    }
}

fn for_init_sets_zero<'a>(init: &Output<'a>) -> bool {
    match init.1.as_ref() {
        Expression::Fragment(children) if children.len() == 2 => {
            matches!(children[1].1.as_ref(), Expression::Integer(0))
        }
        Expression::Assignment(lhs, rhs) => {
            matches!(lhs.1.as_ref(), Expression::Identifier(_))
                && matches!(rhs.1.as_ref(), Expression::Integer(0))
        }
        _ => false,
    }
}

fn for_step_increments_by_one<'a>(step: &Output<'a>) -> bool {
    match step.1.as_ref() {
        Expression::Assignment(lhs, rhs) => {
            let Expression::Identifier(name) = lhs.1.as_ref() else {
                return false;
            };
            match rhs.1.as_ref() {
                Expression::Add(i, one) => {
                    matches!(i.1.as_ref(), Expression::Identifier(n) if *n == *name)
                        && matches!(one.1.as_ref(), Expression::Integer(1))
                }
                Expression::Adjust { .. } => false,
                _ => false,
            }
        }
        Expression::Adjust { op, prefix: true, target } => {
            use parser::ast::AdjustOp;
            matches!(op, AdjustOp::Inc)
                && matches!(target.1.as_ref(), Expression::Identifier(_))
        }
        _ => false,
    }
}

/// Range `start..end` inclusive/exclusive trip count (cap 8).
pub fn range_trip_count<'a>(
    start: &Output<'a>,
    end: &Output<'a>,
    inclusive: bool,
) -> Option<u32> {
    let ConstValue::Int(s) = eval_expr(start, &HashMap::new())? else {
        return None;
    };
    let ConstValue::Int(e) = eval_expr(end, &HashMap::new())? else {
        return None;
    };
    let count = if inclusive {
        e.saturating_sub(s).saturating_add(1)
    } else {
        e.saturating_sub(s)
    };
    if count < 0 || count > 8 {
        return None;
    }
    Some(count as u32)
}

/// Body contains break/continue — skip unroll.
pub fn body_has_loop_control<'a>(body: &Output<'a>) -> bool {
    body_has_loop_control_walk(body)
}

fn body_has_loop_control_walk<'a>(node: &Output<'a>) -> bool {
    use parser::ast::Expression;
    match node.1.as_ref() {
        Expression::Break | Expression::Continue => true,
        Expression::Block(children) => children.iter().any(body_has_loop_control_walk),
        Expression::Fragment(children) => children.iter().any(body_has_loop_control_walk),
        Expression::ExprStatement(inner)
        | Expression::Statement(inner)
        | Expression::Expr(inner)
        | Expression::Group(inner) => body_has_loop_control_walk(inner),
        Expression::If(branches) => branches.iter().any(|b| {
            if let Expression::Branch(cond, body) = b.1.as_ref() {
                cond.as_ref().is_some_and(body_has_loop_control_walk) || body_has_loop_control_walk(body)
            } else {
                false
            }
        }),
        Expression::Match { scrutinee, arms } => {
            body_has_loop_control_walk(scrutinee)
                || arms.iter().any(|arm| body_has_loop_control_walk(&arm.body))
        }
        Expression::Loop { body, .. } => body_has_loop_control_walk(body),
        Expression::For { body, .. } => body_has_loop_control_walk(body),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parser::ast::Expression;

    fn int_expr(n: i64) -> Output<'static> {
        (SimpleSpan::from(0..1), Box::new(Expression::Integer(n)))
    }

    fn id_expr(name: &'static str) -> Output<'static> {
        (SimpleSpan::from(0..1), Box::new(Expression::Identifier(name)))
    }

    #[test]
    fn fold_add_and_cmp() {
        let env = HashMap::new();
        let add = (
            SimpleSpan::from(0..3),
            Box::new(Expression::Add(int_expr(5), int_expr(5))),
        );
        assert_eq!(eval_expr(&add, &env), Some(ConstValue::Int(10)));
        let cmp = (
            SimpleSpan::from(0..3),
            Box::new(Expression::Le(int_expr(4), int_expr(5))),
        );
        assert_eq!(eval_expr(&cmp, &env), Some(ConstValue::Bool(true)));
    }

    /// `Expression::Le` is `<` (not `<=`). Equality must stay false.
    #[test]
    fn le_is_strict_less_than_not_leq() {
        let env = HashMap::new();
        let eq_boundary = (
            SimpleSpan::from(0..3),
            Box::new(Expression::Le(int_expr(5), int_expr(5))),
        );
        assert_eq!(
            eval_expr(&eq_boundary, &env),
            Some(ConstValue::Bool(false)),
            "`5 < 5` must fold to false (Le is strict <)"
        );
        let leq = (
            SimpleSpan::from(0..3),
            Box::new(Expression::Leq(int_expr(5), int_expr(5))),
        );
        assert_eq!(
            eval_expr(&leq, &env),
            Some(ConstValue::Bool(true)),
            "`5 <= 5` must fold to true"
        );
    }

    #[test]
    fn fold_strict_lt_boundary() {
        let env = HashMap::new();
        let cmp = (
            SimpleSpan::from(0..3),
            Box::new(Expression::Le(int_expr(5), int_expr(5))),
        );
        assert_eq!(eval_expr(&cmp, &env), Some(ConstValue::Bool(false)));
    }

    #[test]
    fn const_ident_in_env() {
        let mut env = HashMap::new();
        env.insert("x".into(), ConstValue::Int(5));
        let id: Output = (SimpleSpan::from(0..1), Box::new(Expression::Identifier("x")));
        let add = (
            SimpleSpan::from(0..3),
            Box::new(Expression::Add(id, int_expr(5))),
        );
        assert_eq!(eval_expr(&add, &env), Some(ConstValue::Int(10)));
    }

    #[test]
    fn div_and_mod_by_zero_do_not_fold() {
        let env = HashMap::new();
        let div0 = (
            SimpleSpan::from(0..3),
            Box::new(Expression::Div(int_expr(10), int_expr(0))),
        );
        let mod0 = (
            SimpleSpan::from(0..3),
            Box::new(Expression::Mod(int_expr(10), int_expr(0))),
        );
        assert_eq!(eval_expr(&div0, &env), None);
        assert_eq!(eval_expr(&mod0, &env), None);
    }

    #[test]
    fn strength_mul_int_only_powers_of_two() {
        assert_eq!(strength_mul_int(8), Some(3));
        assert_eq!(strength_mul_int(1), Some(0));
        assert_eq!(strength_mul_int(6), None);
        assert_eq!(strength_mul_int(0), None);
        assert_eq!(strength_mul_int(-4), None);
    }

    #[test]
    fn strength_reduced_inner_add_zero_and_mul_one() {
        let add0 = (
            SimpleSpan::from(0..3),
            Box::new(Expression::Add(id_expr("x"), int_expr(0))),
        );
        let mul1 = (
            SimpleSpan::from(0..3),
            Box::new(Expression::Mul(int_expr(1), id_expr("x"))),
        );
        assert!(matches!(
            strength_reduced_inner(&add0).map(|e| e.1.as_ref()),
            Some(Expression::Identifier("x"))
        ));
        assert!(matches!(
            strength_reduced_inner(&mul1).map(|e| e.1.as_ref()),
            Some(Expression::Identifier("x"))
        ));
    }

    #[test]
    fn for_loop_trip_count_le_and_leq() {
        let init = (
            SimpleSpan::from(0..5),
            Box::new(Expression::Fragment(vec![id_expr("i"), int_expr(0)])),
        );
        let step = (
            SimpleSpan::from(0..5),
            Box::new(Expression::Assignment(
                id_expr("i"),
                (
                    SimpleSpan::from(0..3),
                    Box::new(Expression::Add(id_expr("i"), int_expr(1))),
                ),
            )),
        );
        let le_cond = (
            SimpleSpan::from(0..3),
            Box::new(Expression::Le(id_expr("i"), int_expr(3))),
        );
        let leq_cond = (
            SimpleSpan::from(0..3),
            Box::new(Expression::Leq(id_expr("i"), int_expr(2))),
        );
        assert_eq!(
            for_loop_trip_count(Some(&init), &le_cond, Some(&step)),
            Some(3)
        );
        assert_eq!(
            for_loop_trip_count(Some(&init), &leq_cond, Some(&step)),
            Some(3)
        );
        let too_many = (
            SimpleSpan::from(0..3),
            Box::new(Expression::Le(id_expr("i"), int_expr(9))),
        );
        assert_eq!(
            for_loop_trip_count(Some(&init), &too_many, Some(&step)),
            None
        );
    }

    #[test]
    fn range_trip_count_exclusive_and_inclusive() {
        assert_eq!(
            range_trip_count(&int_expr(0), &int_expr(3), false),
            Some(3)
        );
        assert_eq!(
            range_trip_count(&int_expr(0), &int_expr(2), true),
            Some(3)
        );
        assert_eq!(range_trip_count(&int_expr(0), &int_expr(9), false), None);
    }

    #[test]
    fn body_has_loop_control_detects_break() {
        let plain = (
            SimpleSpan::from(0..1),
            Box::new(Expression::Block(vec![int_expr(1)])),
        );
        let with_break = (
            SimpleSpan::from(0..1),
            Box::new(Expression::Block(vec![(
                SimpleSpan::from(0..1),
                Box::new(Expression::Break),
            )])),
        );
        assert!(!body_has_loop_control(&plain));
        assert!(body_has_loop_control(&with_break));
    }

    #[test]
    fn string_add_folds_concatenation() {
        let env = HashMap::new();
        let lhs = (
            SimpleSpan::from(0..1),
            Box::new(Expression::String("he")),
        );
        let rhs = (
            SimpleSpan::from(0..1),
            Box::new(Expression::String("llo")),
        );
        assert_eq!(
            eval_string_add(&lhs, &rhs, &env),
            Some(ConstValue::Str("hello".into()))
        );
    }
}
