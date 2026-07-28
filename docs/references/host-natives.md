# Host embedder API

Advanced: register Rust closures callable from bytecode without a `.so` file.

### Rust API

```rust
use compiler::pipeline::Pipeline;
use machine::ffi::{FfiSignature, FfiSignatureBuilder};
use machine::memory::{FfiType, Heap};
use common::Value;

let mut pipeline = Pipeline::default();

let sig = FfiSignatureBuilder::new("my_add")
    .arg(FfiType::Int)
    .arg(FfiType::Int)
    .ret(FfiType::Int)
    .build()
    .unwrap();

pipeline.register_host_native(sig, |heap: &mut Heap, args: &[Value]| {
    let sum = args[0].as_int() + args[1].as_int();
    Ok(Some(Value::from(sum)))
});

// After compile:
pipeline.wire_host_natives(&mut vm);
vm.run_raw(&bytecode);
```

### Workflow

| Step | API |
|------|-----|
| Register type + closure | `Pipeline::register_host_native(sig, closure)` |
| Typecheck user calls | Signatures forwarded to HM checker via `Compiler::register` |
| Wire before run | `Pipeline::wire_host_natives(&mut vm)` |
| Bytecode opcode | `HostInvoke` |

### Metadata-only registration

`Pipeline::register_native_function(name, namespace, sig)` registers types without a closure — for tooling or deferred wiring.

### When to use

| Approach | Use when |
|----------|----------|
| Host natives | Embedding coil in a Rust app; hot callbacks; sandboxed API surface |
| `extern` / `dload` | Calling existing C libraries; plugins as `.so` files |

---

## Related

- [ffi](ffi.md)
