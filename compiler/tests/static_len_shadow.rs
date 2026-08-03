use common::Instruction;
use compiler::Compiler;
use parser::Pratt;

fn compile_tests(src: &str) -> (Compiler, Vec<common::Byte>) {
    let mut ast = Pratt::default().parse(src).expect("parse");
    let mut c = Compiler::default();
    c.set_include_tests(true);
    let bc = c.compile("", &mut ast);
    assert!(
        c.get_messages().is_empty(),
        "{:?}",
        c.get_messages()
    );
    (c, bc)
}

/// Later tests binding the same local name must not poison `static_len_of`
/// for earlier tests (span-cached ident types, not flat name map).
#[test]
fn static_len_ignores_later_test_shadowing_same_name() {
    let src = r#"
test("len of literal") {
    let a = [1, 2, 3, 4];
    assert(len(a) == 4)?;
}
test("nested array index") {
    let a = [[1, 2], [3, 4]];
    assert(a[0][1] == 2)?;
}
"#;
    let (c, bc) = compile_tests(src);
    let f0 = c.get_function("__zs_test_0");
    let f1 = c.get_function("__zs_test_1");
    let body = &bc[f0..f1];
    // static_len path: LOAD a; POP; CONST <len>
    let mut saw_len_const = false;
    for w in body.windows(3) {
        if matches!(*w[0].bytecode(), Instruction::LOAD)
            && matches!(*w[1].bytecode(), Instruction::POP)
            && matches!(*w[2].bytecode(), Instruction::CONST)
        {
            assert_eq!(
                w[2].operand_u32(),
                4,
                "len(a) must const-fold to 4, not later nested outer len"
            );
            saw_len_const = true;
            break;
        }
    }
    assert!(saw_len_const, "expected static len LOAD/POP/CONST sequence");
}
