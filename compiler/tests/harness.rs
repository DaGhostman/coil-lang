use common::{Byte, MessageKind};
use compiler::Compiler;
use parser::Pratt;

pub struct TestResult {
    pub bytecode: Vec<Byte>,
    pub messages: Vec<String>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl TestResult {
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    pub fn has_message_containing(&self, text: &str) -> bool {
        self.messages.iter().any(|m| m.contains(text))
    }

    pub fn has_error_containing(&self, text: &str) -> bool {
        self.errors.iter().any(|e| e.contains(text))
    }
}

pub fn compile_source(source: &str) -> TestResult {
    let parser = Pratt::default();
    let mut compiler = Compiler::default();

    let result = match parser.parse(source) {
        Ok(ast) => {
            let bytecode = compiler.compile("", &ast);
            let messages: Vec<String> = compiler
                .get_messages()
                .iter()
                .map(|m| m.message().to_string())
                .collect();

            let errors: Vec<String> = compiler
                .get_messages()
                .iter()
                .filter(|m| *m.kind() == MessageKind::ERROR)
                .map(|m| m.message().to_string())
                .collect();

            let warnings: Vec<String> = compiler
                .get_messages()
                .iter()
                .filter(|m| *m.kind() == MessageKind::WARNING)
                .map(|m| m.message().to_string())
                .collect();

            TestResult {
                bytecode,
                messages,
                errors,
                warnings,
            }
        }
        Err(e) => TestResult {
            bytecode: Vec::new(),
            messages: vec![e.message().to_string()],
            errors: vec![e.message().to_string()],
            warnings: Vec::new(),
        },
    };

    result
}

pub fn compile_and_check_no_errors(source: &str) -> TestResult {
    let result = compile_source(source);
    assert!(
        !result.has_errors(),
        "Expected no errors, but got: {:?}",
        result.errors
    );
    result
}

pub fn compile_and_expect_error(source: &str, expected_error: &str) -> TestResult {
    let result = compile_source(source);
    assert!(
        result.has_error_containing(expected_error),
        "Expected error containing '{}', but got errors: {:?}",
        expected_error,
        result.errors
    );
    result
}

pub fn count_instruction(bytecode: &[Byte], instruction: common::Instruction) -> usize {
    bytecode
        .iter()
        .filter(|b| b.bytecode() == &instruction)
        .count()
}

pub fn find_function_offset(_bytecode: &[Byte], _name: &str) -> Option<usize> {
    None
}
