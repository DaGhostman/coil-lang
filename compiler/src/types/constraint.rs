use std::collections::HashMap;

use parser::SimpleSpan;

use super::type::Type;

/// Type constraint for Hindley-Milner unification
#[derive(Clone, Debug)]
pub struct Constraint {
    pub left: Type,
    pub right: Type,
    pub span: SimpleSpan,
}

impl Constraint {
    pub fn new(left: Type, right: Type, span: SimpleSpan) -> Self {
        Self {
            left,
            right,
            span,
        }
    }
}

/// Set of constraints for type checking
#[derive(Debug, Default)]
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
    pub fn solve(&self) -> Result<HashMap<String, Type>, Vec<String>> {
        // For now, return empty solution
        // This will be implemented with full HM unification
        Ok(HashMap::new())
    }

    /// Check all constraints and return any errors
    pub fn check(&self) -> Vec<String> {
        // For now, return empty vector
        // This will be implemented with full constraint checking
        Vec::new()
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