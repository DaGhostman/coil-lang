# Error codes

Diagnostics carry a stable `ErrorCode` (shown as `E####` in pretty output,
SARIF `ruleId`, and LSP `code`). Codes are grouped by family:

| Range | Family |
|-------|--------|
| `E00xx` | Parse / syntax |
| `E01xx` | Name resolution & types |
| `E02xx` | Enums, match, constructs |
| `E03xx` | Format strings & builtins |
| `E04xx` | Aggregates, records, FFI tags |
| `E08xx` | Codegen |
| `E09xx` | CLI / I/O / archive |

## Catalog

| Code | Name | Meaning |
|------|------|---------|
| `E0001` | `ParseError` | Syntax / parse error |
| `E0100` | `UnknownValue` | Cannot find value in this scope |
| `E0101` | `UnknownFunction` | Cannot find function |
| `E0102` | `TypeMismatch` | Type mismatch |
| `E0103` | `InfiniteType` | Infinite type (occurs check) |
| `E0104` | `NotAFunction` | Value is not callable |
| `E0105` | `TooManyArguments` | Too many arguments |
| `E0106` | `UndeclaredAssignment` | Assignment to undeclared variable |
| `E0107` | `InvalidAssignment` | Invalid assignment target |
| `E0108` | `VariableRedeclaration` | Variable redeclaration |
| `E0109` | `ConstantRedeclaration` | Constant redeclaration |
| `E0110` | `UnknownType` | Unknown type name |
| `E0111` | `ReturnMismatch` | Return type mismatch |
| `E0112` | `YieldOutsideAsync` | `yield` outside `async fn` |
| `E0113` | `ResumeTypeMismatch` | Resume / send type mismatch |
| `E0114` | `InvalidTry` | `?` on non-Option/non-Result, or outside a function |
| `E0115` | `InvalidCoalesce` | `??` on non-Option/non-Result |
| `E0116` | `InvalidOptionalAccess` | `?.` on non-Option (e.g. Result) |
| `E0117` | `ConflictingErrorType` | Mixed Option/Result modes or conflicting `E` in one function |
| `E0118` | `UnreachableCode` | Code after a diverging statement (warning) |
| `E0119` | `GenericTypeError` | Other type error |
| `E0120`–`E0122` | Overload family | Wrong arity / duplicate / ambiguous overload |
| `E0123` | `DeferNeverRuns` | `defer` never runs on function exit (warning) |
| `E0124` | `WildcardImport` | Userland (disk) `use path::*` — list names explicitly; virtual modules may still glob |
| `E0200`–`E0212` | Enum / match family | Duplicate enum, unknown variant, non-exhaustive match, field errors, … |
| `E0300`–`E0301` | Format family | Specifier / arity mismatch |
| `E0400`–`E0405` | Aggregate / FFI family | Index OOB, array element mismatch, `declare`/`invoke` arity, … |
| `E0800`–`E0801` | Codegen family | Unknown expression / codegen error |
| `E0900` | `IoError` | I/O failure |
| `E0901` | `ArchiveVersionMismatch` | Stale `.hyc` archive |
| `E0902` | `InvalidCliFlags` | Conflicting or unknown CLI flags |
| `E0903` | `MissingInputFile` | No input file given |

## CLI output modes

| Flag | Format | Stream |
|------|--------|--------|
| (default) | Pretty (ariadne) | stderr |
| `--log-json` | SARIF 2.1 | stdout |
| `--log-lsp` | LSP Diagnostic NDJSON | stdout |

`--log-json` and `--log-lsp` are mutually exclusive.
