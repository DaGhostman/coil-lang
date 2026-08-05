test("string literal coerces to byte") {
    let slash: byte = "/";
    let nl: byte = "\n";
    assert((slash as int) == 47)?;
    assert((nl as int) == 10)?;
    assert(slash == "/")?;
    let buf: [byte] = ["H", "i", "\n"];
    assert((buf[0] as int) == 72)?;
    assert(("." as byte) as int == 46)?;
}

test("escaped quote string literal as byte") {
    let q: byte = "\"";
    assert((q as int) == 34)?;
}
