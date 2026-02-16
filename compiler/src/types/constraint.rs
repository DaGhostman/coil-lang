use super::ty::Type;
use crate::types::unify::unify_types;
use parser::SimpleSpan;

/// Type constraint for Hindley-Milner unification
#[derive(Clone)]
pub struct Constraint {
    pub left: Type,
    pub right: Type,
    pub span: SimpleSpan,
}

impl Constraint {
    pub fn new(left: Type, right: Type, span: SimpleSpan) -> Self {
        Self { left, right, span }
    }
}

/// Set of constraints for type checking
#[derive(Default)]
pub struct ConstraintSet {
    pub constraints: Vec<Constraint>,
}

impl ConstraintSet {
    /// Create a new empty constraint set
    pub fn new() -> Self {
        Self {
            constraints: Vec::new(),
        }
    }

    /// Add a new constraint
    pub fn add(&mut self, left: Type, right: Type, span: SimpleSpan) {
        self.constraints.push(Constraint::new(left, right, span));
    }

    /// Add a constraint from the current constraint set
    pub fn add_constraint(&mut self, constraint: Constraint) {
        self.constraints.push(constraint);
    }

    /// Solve the constraint set using Hindley-Milner unification
    pub fn solve(&self) -> Result<crate::types::substitution::Substitution, Vec<String>> {
        use crate::types::substitution::Substitution;

        let mut substitution: Substitution = Substitution::new();
        let mut errors: Vec<String> = Vec::new();

        for constraint in self.constraints.iter() {
            match unify_types(&constraint.left, &constraint.right, &mut substitution) {
                crate::types::unify::UnifyResult::Success(_) => {
                    // Continue with accumulated substitution
                }
                crate::types::unify::UnifyResult::Failure(msg) => {
                    errors.push(format!(
                        "Constraint violation at {:?}: {}",
                        constraint.span, msg
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(substitution)
        } else {
            Err(errors)
        }
    }

    /// Check all constraints and return any errors
    pub fn check(&self) -> Vec<String> {
        let mut errors: Vec<String> = Vec::new();

        for constraint in self.constraints.iter() {
            // Basic check: are the types structurally equal?
            if constraint.left != constraint.right {
                errors.push(format!(
                    "Type mismatch at {:?}: expected {}, found {}",
                    constraint.span, constraint.left, constraint.right
                ));
                
                // Add helpful suggestion
                let left_name = constraint.left.type_name();
                let right_name = constraint.right.type_name();
                if left_name == "int" && right_name == "float" || left_name == "float" && right_name == "int" {
                    errors.push(format!(
                        "  Note: Numeric type coercion is not implicit in Zero-Script. Consider using an explicit cast."
                    ));
                }
            }
        }

        errors
    }

    /// Get all constraints
    pub fn get_constraints(&self) -> &[Constraint] {
        &self.constraints
    }

    /// Get constraint count
    pub fn len(&self) -> usize {
        self.constraints.len()
    }

    /// Check if constraint set is empty
    pub fn is_empty(&self) -> bool {
        self.constraints.is_empty()
    }
}
