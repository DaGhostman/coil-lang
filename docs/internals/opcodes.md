# VM opcodes (builtins-related)

User code does not name these directly; the compiler emits them:

| Opcode | Role |
|--------|------|
| `PRINT` | Write string to output |
| `FORMAT` | Build formatted string from specifiers |
| `FfiLoad` | `dload` |
| `DeclareFFI` | `declare` |
| `FfiInvoke` | `invoke` |
| `HostInvoke` | Host-registered closure |
| `HostInvokeNiche` | Allocation-free niche `Option<T>` Vec native |
| `OptionNicheToHeap` / `HeapOptionToNiche` | Cross a pointer-niche `Option<T>` boundary |
| `PairToHeap` / `HeapToPair` | Box or unbox a unary `[payload, tag]` pair |
| `ReturnPair` | Return a unary pair without changing `Value` |
| `Panic` | Abort after writing `panic: <msg>` |

---

## Related

- [pipeline](pipeline.md)
- [debug-info](debug-info.md)
