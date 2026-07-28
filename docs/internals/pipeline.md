# Compilation and execution pipeline

How a `.hy` program becomes running bytecode on the VM.

## Stages

1. **Parse** — the Pratt parser in `parser/` reads `.hy` source into an AST.
2. **Typecheck** — Algorithm W (Hindley–Milner) in `compiler/src/typechecking/` infers types and reports source-anchored diagnostics via ariadne.
3. **Codegen** — single-pass stack codegen in `compiler/src/lib.rs` emits bytecode, then a peephole fusion pass (`compiler/src/peephole.rs`) may collapse common convoys. Fusion must keep `debug_locs` in sync via `shrink_debug_locs`.
4. **Archive** — bytecode is wrapped in a versioned `ArchivedProgram` envelope (`ARCHIVE_VERSION` is currently **30** in `common/src/archive.rs`) and written to `out.hyc` (or another path via `compile -o`). See [Debug line table](debug-info.md).
5. **Execute** — the VM in `machine/` loads the archive and runs `main` (or each `test("…")` case under `coil test`).

## Cache and rebuild

Re-run the same CLI entry without deleting `out.hyc` to reuse the cached compile. Delete `out.hyc` (or bump the archive version) to force a fresh compile. The CLI recompiles automatically when the archive is missing, corrupt, version-mismatched, older than any recorded source (including `use`d modules), or was built for a different entry file.

## Multi-file programs

With a `coil.toml`, the pipeline discovers dependencies via `use` / `mod`, compiles each file with a namespace prefix, and links them into one archive. The **entry file** uses the empty namespace. See [Modules](../references/modules.md) and [Project config](../references/project-config.md).

## Opcode discipline

New `Instruction` variants are **append-only** (preserve `#[repr(u8)]` discriminants). Incompatible changes bump `ARCHIVE_VERSION`. The release VM `promise!` ceiling must track the last variant. Selected builtin-related opcodes are listed in [opcodes.md](opcodes.md).
