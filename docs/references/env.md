# `env` module

`use env::{args, var, cwd, exit, exec};` — `args()` returns `Result<Vec<string>, EnvError>` (`Ok` with argv including argv0). `var` / `set_var` / `remove_var`, `cwd` / `set_cwd`, `exit(code)`. `exec(program, args)` spawns a program with an argv vector (no shell). The child inherits the VM process **cwd** and **environment**; there are no per-call overrides yet. The compiler emits a **warning** when `exec` or `exit` is used. **Only `exec` is runtime-gated:** by default it returns `EnvError::ExecDisabled` unless `coil.toml` `[env] allow_exec = true`. `exit` is compile-warned only (not blocked at runtime).

---
