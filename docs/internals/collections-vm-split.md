# Collections: VM hoist vs userland

Plan for `HashMap`, `HashSet`, `List`, and `TreeMap` in the standard library.

## Already on the VM / compiler (no new opcodes)

| Primitive | Role for collections |
|-----------|----------------------|
| `Hash` / `Eq` / `Ord` traits | Key constraints (`hash()`, `==`, `<` / `>`) |
| Dynamic arrays `[T]`, `arr[] =`, `len` | Buckets, chains, growable storage |
| Classes + field mutation | Mutable map / list / set handles |
| Recursive enums | Persistent trees / functional lists |
| Bit ops (`&`, `<<`) and `%` | Bucket index from hash |
| `HostInvoke` (not opcodes) | Prefer this over new opcodes if a native ever lands |
| `#[max_depth(N)]` | Bound recursive tree / list walks |

**Do not add** map/set/list opcodes or a heap `Object::HashMap`. That would be benchmark-shaped surface area; AGENTS prefers alloc reduction and `HostInvoke` over new opcodes unless the pattern is universal.

## Userland (this change)

| Type | Module | Representation |
|------|--------|----------------|
| `HashMap<K,V>` | `collections::map` | Separate chaining: `heads: [int]` + parallel `keys` / `vals` / `next` / `live` |
| `HashSet<T>` | `collections::map` | `HashMap<T, bool>` wrapper (same module — userland class types are not importable across modules yet) |
| `List<T>` | `collections::list` | Mutable singly-linked `Node` class (`Option<Node<T>>`) |
| `TreeMap<K,V>` | `collections::tree` | Mutable BST via parallel arrays + child indices (avoids `Option` field moves) |

Constrained ops (`insert` / `get_or` / …) are **free functions** with `T: Eq + Hash` or `T: Ord + Eq` bounds. Inherent `impl` bounds are parsed but not applied to method schemes (see below), and inherent method calls do not yet emit dictionary arguments.

**`Option` field caveat:** matching a class field of type `Option<_>` moves the value out — write it back (`t.root = Option::Some(root)`) before returning, and copy child links to a `let` before nested `match` so the field is restored.

## Known language gaps (hoist candidates)

| Gap | Impact | Recommended hoist |
|-----|--------|-------------------|
| `impl Foo<T: Eq>` bounds ignored in `infer_impl` | Methods cannot use `==` / `.hash()` on `T` | Apply binder bounds to `active_constraints` + `Scheme::poly` + `fn_dict_arity` |
| Inherent method `CALL` skips dict args | Even with fixed schemes, `m.insert(k,v)` would not pass `Eq`/`Hash` dictionaries | Mirror free-fn `emit_call_site_dicts` on method calls |
| `[Option<T>]` / `[Foo<K,V>]` parse error | No array-of-generic without a `type` alias | Parser: allow nested generics in array element types |
| Free `fn f<T>(T) -> Option<T>` corrupts string/int payloads | Blocks `get → Option` as a free fn | Codegen/unbox for generic enum returns (methods returning `Option` are OK) |
| Functional `List` recursion can panic on stack | Prefer mutable class list for now | VM stack / `max_depth` interaction audit |

## Future (only if measured)

- `HostInvoke` batch helpers (e.g. rehash) if userland grow shows up in profiles.
- Native open-addressing table as an opaque heap object — only with a cross-cutting need (serde, runtime internals), not for microbenchmarks.

## API shape

```coil
use collections::map::{HashMap, hashmap_new, hashmap_insert, hashmap_get_or, hashset_new};
use collections::list::{List, list_new, list_push_front, list_pop_front_or};
use collections::tree::{TreeMap, treemap_new, treemap_insert, treemap_get_or};
```

Existing `collections::{sort, reverse, collect_ints, …}` stays in `stdlib/collections.hy`.
