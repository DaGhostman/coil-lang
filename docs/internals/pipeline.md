# Compilation and execution pipeline

How a `.hy` program becomes running bytecode on the VM.

## Stages

1. **Parse** — the Pratt parser in `parser/` reads `.hy` source into an AST.
2. **Typecheck** — Algorithm W (Hindley–Milner) in `compiler/src/typechecking/` infers types and reports source-anchored diagnostics via ariadne.
3. **Codegen (stack IL)** — `compiler/src/lib.rs` emits a compile-time stack IL (`compiler/src/il/`) with **symbolic labels** for jumps/joins. Control flow uses `IlOp::Jump` / `IlOp::Label` instead of absolute PCs.
4. **Lower** — after multi-file link, `finalize_bytecode` runs IL opts (`jump_thread`, `dead_block` after JMP/RETURN, `stack_dce`, **`return_convoy`**: identical immediate `LOAD`/`CONST` into a return-label cluster → `*Return`) then `il::lower`: fuse-select with **label / abs-jump barriers** (plus `*Return` join-on-window[0]), assign PCs once, encode `Vec<Byte>`. Enabled fuse patterns: const fold, `BinSlotImm` / `BinSlotSlot`, `CmpJmpf` / `LogNotJmpf` / `BinSlotImmJmpf`, `LoadReturnSlot` / `ConstReturnImm` / `BinReturn`. CALL/TailCall/MakeCoro/CodePtr/MakePolyFn use `IlOp::Entry`; production abs JMP is rejected in lower — no post-lower peephole/`adjust_target` hot path. Tiny direct-call inlining judges candidacy on **IL emitting ops** and can expand a sole `ConstReturnImm`/`LoadReturnSlot` body.
5. **Archive** — bytecode is wrapped in a versioned `ArchivedProgram` envelope (`ARCHIVE_VERSION` is currently **30** in `common/src/archive.rs`) and written to `out.hyc` (or another path via `compile -o`). See [Debug line table](debug-info.md).
6. **Execute** — the VM in `machine/` loads the archive and runs `main` (or each `test("…")` case under `coil test`).

```
AST + HM → Stack IL (labels) → IL opts → lower/fuse-select → Vec<Byte> → .hyc → VM
```

The IL is **compile-time only**; the VM and archive format are unchanged.

## Cache and rebuild

Re-run the same CLI entry without deleting `out.hyc` to reuse the cached compile. Delete `out.hyc` (or bump the archive version) to force a fresh compile. The CLI recompiles automatically when the archive is missing, corrupt, version-mismatched, older than any recorded source (including `use`d modules), or was built for a different entry file.

## Multi-file programs

With a `coil.toml`, the pipeline discovers dependencies via `use` / `mod`, compiles each file with a namespace prefix into one shared IL buffer, and **lowers once** after linking. The **entry file** uses the empty namespace. See [Modules](../references/modules.md) and [Project config](../references/project-config.md).

## Opcode discipline

New `Instruction` variants are **append-only** (preserve `#[repr(u8)]` discriminants). Incompatible changes bump `ARCHIVE_VERSION`. The release VM `promise!` ceiling must track the last variant. Selected builtin-related opcodes are listed in [opcodes.md](opcodes.md).
