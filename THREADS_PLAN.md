# Native Threads, ThreadPool, and Channels

## Status

**Design locked** (investigation + user choices). Implementation not yet started.

| Decision | Choice |
|----------|--------|
| Delivery scope | **Full:** `Thread` + `Channel` + `ThreadPool` in one cut |
| Channel payloads | **Deep-copy** almost any coil value; reject opaque host handles that cannot be cloned across heaps |
| Runtime model | **One `Machine` per OS thread** (isolate); no shared VM heap |
| Language surface | Virtual module `thread` via **HostInvoke** (mirror `io`) — no new opcodes |
| `ARCHIVE_VERSION` | **No bump** (HostInvoke-only; append heap `Object` variants only) |

Related docs today: OS threads are explicitly **not** builtins ([`docs/reference/built-ins.md`](docs/reference/built-ins.md)); cooperative coroutines remain orthogonal ([`docs/tutorial/08-coroutines.md`](docs/tutorial/08-coroutines.md)).

---

## Why isolate Machines (not a shared heap)

Current VM assumptions that forbid a shared-heap design without a concurrent GC project:

- Single `Machine` owns `heap`, `stack`, `frames`, `resume_stack`, `statics`, `alloc_counter` ([`machine/src/vm.rs`](machine/src/vm.rs)).
- Stop-the-world GC roots only that machine’s stack + coroutines ([`Machine::gc_collect`](machine/src/vm.rs)).
- `Gc::payload_mut(&self)` is documented **single-threaded VM only** ([`machine/src/memory/heap.rs`](machine/src/memory/heap.rs)).
- Coroutines splice into the **shared** operand stack (`ResumeCtx`); they are cooperative on one runner, not OS threads.
- `Value` is an untagged word (immediate or heap address) with no heap provenance ([`common/src/value.rs`](common/src/value.rs)).

**Chosen model:** each OS thread runs its own `Machine` with a private heap/stack/frames/statics. Threads share only:

1. Immutable program data (bytecode + constant pool + debug locs) — `Arc`-backed.
2. Host native registry (`Arc<dyn NativeFn>` already `Send + Sync`).
3. Host-owned sync objects (`Thread` join state, channel ends, pool workers) living **outside** any coil heap.

Cross-thread communication deep-copies values through a portable intermediate (see § Deep-copy).

```mermaid
flowchart LR
  subgraph parent [Parent OS thread]
    MP[Machine parent]
    HP[Heap parent]
    MP --> HP
  end
  subgraph child [Worker OS thread]
    MC[Machine child]
    HC[Heap child]
    MC --> HC
  end
  Prog[Arc program bytecode]
  Host[Host Thread / Channel / Pool]
  MP -.-> Prog
  MC -.-> Prog
  MP -->|spawn deep_copy Fn| Host
  Host -->|start| MC
  MP <-->|send / recv deep_copy| Host
  MC <-->|send / recv deep_copy| Host
```

---

## User-facing API (locked)

Module path: **`thread`** (explicit `use thread::*;`). Nested optional later; v1 is flat like core `io`.

### Opaque types

| Type | Meaning |
|------|---------|
| `Thread` | Join handle for a spawned OS thread (or pool job) |
| `Sender` | Channel send endpoint |
| `Receiver` | Channel receive endpoint |
| `ThreadPool` | Fixed-size worker pool |

All are `Ty::Con(...)` opaque types (same pattern as `Stream`). Runtime: heap `Object::Thread` / `Object::Sender` / `Object::Receiver` / `Object::ThreadPool` holding `Arc<...>` to host state.

### Error type

Unit enum `ThreadError` (lazy-registered on `use thread::*`, same as `IoError`):

| Variant | When |
|---------|------|
| `WouldBlock` | `try_send` / `try_recv` would wait |
| `Disconnected` | Peer dropped / channel closed |
| `JoinFailed` | Worker panicked or join already consumed |
| `NotSendable` | Deep-copy rejected (opaque / cycle policy / unsupported object) |
| `PoolShutdown` | `submit` after pool shutdown |
| `Other` | Catch-all |

Results: `Result<T, ThreadError>` via existing prelude `Result`.

### Functions

```coil
// --- Thread ---
// Spawn a zero-arg function on a new OS thread. The closure / Fn value
// (and its captures) are deep-copied into the child Machine.
fn spawn(f: () -> T) -> Result<Thread, ThreadError>

// Block until the thread finishes; deep-copy the return value into the
// caller's heap. Consumes the join (second join → JoinFailed).
fn join(t: Thread) -> Result<T, ThreadError>

// Detach: allow the thread to run without join. Further join → JoinFailed.
fn detach(t: Thread) -> Result<(), ThreadError>

// --- Channel (unbounded MPSC host queue; directional) ---
fn channel[T]() -> (Sender, Receiver)

fn send[T](tx: Sender, value: T) -> Result<(), ThreadError>
fn recv[T](rx: Receiver) -> Result<T, ThreadError>          // blocking
fn try_send[T](tx: Sender, value: T) -> Result<(), ThreadError>
fn try_recv[T](rx: Receiver) -> Result<T, ThreadError>      // WouldBlock if empty
fn close(tx: Sender) -> Result<(), ThreadError>             // optional; Drop also closes

// --- ThreadPool ---
fn pool(workers: int) -> Result<ThreadPool, ThreadError>    // workers >= 1
fn submit[T](p: ThreadPool, f: () -> T) -> Result<Thread, ThreadError>
fn shutdown(p: ThreadPool) -> Result<(), ThreadError>       // refuse new submit; join workers
```

Notes:

- **No method syntax required for v1** (free functions + UFCS later if desired). Mirror `io` free fns.
- **`spawn` / `submit` accept first-class `ObjFn` / lambdas** with arity 0 after captures (`() -> T`). Named top-level functions work via existing callable values.
- **Generics:** schemes are polymorphic in `T` the same way IO schemes are monomorphic today — implement via HM type variables in `thread_fn_scheme` (pattern after polymorphic builtins if present; otherwise monomorphize at call site via unification with argument/return). Prefer real polymorphism: `Scheme` with quantified `T`.
- **Statics are per-Machine:** child threads do **not** see parent `static` mutations. Document this.
- **Coroutines stay single-Machine:** `async fn` / `resume` do not cross OS threads. Spawning an `async fn` is rejected (`NotSendable` or type error: spawn expects `() -> T`, not `coroutine<…>`).

### Example sketches

`examples/thread_join.hy` → print `42`:

```coil
use thread::*;

fn work() -> int {
    return 40 + 2;
}

fn main() {
    let t = spawn(work)?;
    print "%i", join(t)?;
}
```

`examples/thread_channel.hy` → print `hello`:

```coil
use thread::*;

fn main() {
    let pair = channel();
    let tx = pair[0];
    let rx = pair[1];
    let t = spawn(|| {
        send(tx, "hello")?;
        return 0;
    })?;
    print "%s", recv(rx)?;
    join(t)?;
}
```

(Exact lambda / tuple destructure syntax must match current grammar; adjust to `let pair = channel();` + index or record if needed.)

`examples/thread_pool.hy` → print sum of parallel jobs.

---

## Runtime architecture

### Host state (new crate module)

New file: [`machine/src/thread.rs`](machine/src/thread.rs) (and `mod thread;` from `machine/src/lib.rs`).

```text
JoinState {
  done: Mutex<Option<Result<PortableValue, String>>>,  // panic message or value
  finished: Condvar,
}

ObjThread payload → Arc<JoinState> + Option<JoinHandle<()>>

ChannelInner {
  queue: Mutex<VecDeque<PortableValue>>,
  closed: AtomicBool / flag under mutex,
  not_empty / not_full: Condvar,  // unbounded → only not_empty needed
}
Sender / Receiver → Arc<ChannelInner> (+ Generation / id for disconnect)

PoolInner {
  workers: Vec<JoinHandle<()>>,
  job_tx: std::sync::mpsc::Sender<Job>,  // Job = Box<dyn FnOnce() + Send>
  shutdown: AtomicBool,
}
```

Worker threads for `spawn` / pool:

1. Build a fresh `Machine` with `Arc` program + cloned host-native list.
2. Deep-copy the entry `ObjFn` (and captures) into the child heap.
3. Invoke the function (reuse nested-call / `call_function` path).
4. Deep-copy the return `Value` into a `PortableValue`; store in `JoinState`.
5. On panic / VM panic flag → `JoinFailed` / error string.

### Making `Machine` movable onto threads

Today `OutputSink = Box<dyn IoWrite>` without `Send` → `Machine: !Send`.

Required plumbing:

- Change output trait object to `Box<dyn IoWrite + Send>` (or `Mutex<Box<dyn IoWrite + Send>>` if parents share a stdout sink).
- Pipeline / test `SharedBuf` adapters must be `Send` (use `Arc<Mutex<Vec<u8>>>` instead of `Rc<RefCell<…>>` where threads need capture).
- Child machines default to inheriting a shared `Arc<Mutex<dyn Write + Send>>` for `print`, or discard output unless configured — **default: share parent stdout via `Arc<Mutex<…>>`** so examples’ `print` from workers is visible and ordered coarsely.

### Program sharing

Add something like:

```rust
pub struct SharedProgram {
    pub code: Arc<[Byte]>,      // or Arc<Vec<Byte>>
    pub constants: Arc<[u64]>,
    pub static_slot_count: u32,
    pub debug: Option<Arc<ProgramDebug>>,
}
```

Parent `run_with_pool` installs `SharedProgram` on the machine. Spawn clones the `Arc`s into the child. No bytecode mutation at runtime today, so sharing is sound.

### Heap object variants

Append to `Object` in [`machine/src/memory/heap.rs`](machine/src/memory/heap.rs):

- `Thread(RefThread)` — `Arc<JoinState>` (+ join handle ownership rules)
- `Sender(RefSender)`, `Receiver(RefReceiver)`
- `ThreadPool(RefPool)`

GC: these hold **no coil `Value` roots** (only `Arc` to host). Mark/size/display arms required; Drop may `detach` / `close` defensively.

**No new opcodes. No `ARCHIVE_VERSION` bump.**

---

## Deep-copy / `PortableValue`

### Portable IR

Host-side enum (not a coil type), roughly:

```rust
enum PortableValue {
    Immediate(u64),           // raw Value bit pattern for immediates / null
    String(String),
    Array(Vec<PortableValue>),
    Tuple(Vec<PortableValue>),
    Enum { tag: u32, payload: Vec<PortableValue> },
    Instance { fields: Vec<(String, PortableValue)> }, // dict / class instance
    Fn {
        entry: u32,
        arity: u32,
        is_rest: bool,
        filled_mask: u32,
        captured_args: Vec<PortableValue>,
        captures: Vec<PortableValue>,
    },
    PolyFn {
        entry: u32,
        type_arity: u8,
        captured_dicts: Vec<Option<PortableValue>>,
    },
    Boxed(PortableValue),
    // Library? reject or share Arc<Library> by path id — see below
}
```

### Copy rules

| Source `Object` | Action |
|-----------------|--------|
| Immediate `Value` | Bit-copy into `Immediate` |
| `String` | Own `String` bytes |
| `Array` / `Tuple` / `Enum` / `Boxed` | Recurse |
| `Instance` | Recurse field table (string keys + members) |
| `Fn` / `PolyFn` | Copy descriptor; recurse captures / dicts; **reuse `entry` offsets** (same shared program) |
| `Library` | Prefer re-`dlopen` / share `Arc<Library>` by registered id; if awkward, `NotSendable` in v1 |
| `Stream` | **`NotSendable`** |
| `Coroutine` | **`NotSendable`** |
| `Thread` / `Sender` / `Receiver` / `ThreadPool` | **Sendable as handle:** encode as host-id in a dedicated portable variant that re-wraps the same `Arc` on the destination heap (not a structural deep-copy) |

Cycle handling: maintain `HashMap<*const (), PortableValue>` (or addr → already-copied) while encoding; on back-edge, either fail `NotSendable` or emit a structured cycle ref. **v1: reject cycles with `NotSendable`** (simpler; document).

Decode: allocate into the **destination** `Heap`, reconstructing objects. Strings go through `heap.intern`.

API:

```rust
pub fn value_to_portable(heap: &Heap, v: Value) -> Result<PortableValue, ThreadErrorKind>;
pub fn portable_to_value(heap: &mut Heap, p: PortableValue) -> Result<Value, ThreadErrorKind>;
```

Used by: `send`/`recv`, `spawn` (copy fn in), `join` (copy result out), `submit`.

### Typechecker sendability (best-effort)

Static check in `spawn` / `send` / `submit` schemes:

- Reject known-unsound types when fully resolved: `Stream`, `coroutine<…>`, and (if distinguishable) IO handles.
- `Thread` / `Sender` / `Receiver` / `ThreadPool` **are** allowed (handle share).
- Open type variables: allow; runtime deep-copy is the source of truth (`NotSendable`).

Mirror FFI’s `is_ffi_marshallable_ty` as `is_thread_sendable_ty` in [`infer.rs`](compiler/src/typechecking/infer.rs).

---

## Compiler / typechecker / pipeline checklist

Follow the `io` layering exactly.

| Layer | Work |
|-------|------|
| [`common/src/builtins.rs`](common/src/builtins.rs) | `ThreadError` name + variant list; reserve enum name |
| [`compiler/src/typechecking/virtual_modules.rs`](compiler/src/typechecking/virtual_modules.rs) | `THREAD_MODULE`, `ThreadBuiltin` enum, exports (`OpaqueType` + `ThreadFn` / reuse host-fn kind), `native_name()` prefixes (`thread_spawn`, …) |
| [`compiler/src/typechecking/ty.rs`](compiler/src/typechecking/ty.rs) | `thread_ty()`, `sender_ty()`, `receiver_ty()`, `thread_pool_ty()` |
| [`compiler/src/typechecking/infer.rs`](compiler/src/typechecking/infer.rs) | Lazy `ThreadError` registration; `thread_fn_scheme`; `thread_fn_in_scope`; `parse_type_name_str` for opaque names; sendability checks |
| [`compiler/src/lib.rs`](compiler/src/lib.rs) | Call arm: if `thread_fn_in_scope` → same HostInvoke emission as IO (generalize `emit_io_host_invoke` → `emit_host_invoke(native_name, args)`) |
| [`compiler/src/pipeline.rs`](compiler/src/pipeline.rs) | `register_thread_natives()` from `Pipeline::new()`; id-only registration (do **not** pollute FFI type env) |
| Heap / VM | New `Object` variants; no opcode / archive changes |
| [`machine/src/thread.rs`](machine/src/thread.rs) | Host impls + portable deep-copy |
| Docs | Tutorial `11-threads.md`; update `built-ins.md`, `modules.md`, `types.md`, `README.md` feature matrix, `examples.md` |
| Examples + goldens | `examples/thread_*.hy` + pipeline tests |
| `AGENTS.md` | Learned fact: virtual `thread` module; isolate Machines; deep-copy channels |

---

## ThreadPool semantics (locked)

- `pool(n)` creates `n` OS threads, each with a **long-lived** `Machine` that pulls jobs from a host queue **or** spawns a fresh Machine per job. **Prefer fresh Machine per job** for isolation (no leftover locals/statics between jobs); workers are OS threads that construct a Machine, run one job, tear down. (Cheaper pooling of Machines can be a later optimization.)
- `submit(p, f)` deep-copies `f` into a `PortableValue`, enqueues; a worker decodes onto its Machine, runs, stores result in a `JoinState`, returns `Thread` (= join handle).
- `shutdown(p)` stops accepting jobs, waits for queue drain + workers; further `submit` → `PoolShutdown`.
- Drop of last `ThreadPool` handle triggers shutdown asynchronously or blocks — **v1: `shutdown` is explicit; Drop calls shutdown + join workers**.

---

## Interaction with existing features

| Feature | Interaction |
|---------|-------------|
| Coroutines | Per-Machine only; not sendable |
| FFI / `libffi` | Child Machine gets same host natives + can `dload` independently; avoid sharing raw `*mut Heap` across threads (already single-thread documented) |
| `io` streams | Not sendable; open files inside the worker instead |
| `print` | Shared `Send` stdout sink |
| `static` | Per-Machine; not a cross-thread broadcast |
| GC | Unchanged stop-the-world **per Machine** |
| Peephole / codegen | Unaffected |

---

## Testing plan

### Unit (machine)

- `portable_roundtrip_immediate_string_array_tuple_enum_instance_fn`
- `portable_rejects_stream_and_coroutine`
- `portable_shares_sender_handle_identity`
- `spawn_join_returns_deep_copied_int`
- `channel_send_recv_across_two_machines`
- `try_recv_would_block`
- `recv_after_close_disconnected`
- `pool_submit_n_jobs_join_all`
- `submit_after_shutdown_errors`

### Typechecker / diagnostics

- `spawn` on non-function / wrong arity
- `send` of `Stream` → diagnostic or runtime `NotSendable` golden
- Unknown `thread::` import without `use`

### Pipeline goldens

- `example_thread_join_prints_42`
- `example_thread_channel_prints_hello`
- `example_thread_pool_prints_sum`

Run with 64MB memory limit per project preference when exercising pools (watch for leaks of `JoinState` / Machines).

---

## Implementation phases (within the Full cut)

Work can land as stacked commits on one PR, but all ship together:

1. **Plumbing:** `PortableValue` deep-copy; `SharedProgram`; `Send` output sink; heap object stubs.
2. **Thread:** `spawn` / `join` / `detach` + virtual module + example.
3. **Channel:** `channel` / `send` / `recv` / `try_*` + example.
4. **ThreadPool:** `pool` / `submit` / `shutdown` + example.
5. **Docs + AGENTS.md + diagnostics polish.**

---

## Explicit non-goals (this cut)

- Shared-heap concurrent GC / true shared `Value` pointers across OS threads.
- Bounded channels with back-pressure sizing API (`channel(capacity)` — easy follow-up; v1 unbounded).
- `Select` / multi-recv.
- Structured concurrency / cancellation tokens.
- Making coroutines migrate across OS threads.
- `async`/`await` sugar over threads.
- Work-stealing pool or Machine reuse optimization.

---

## Risk register

| Risk | Mitigation |
|------|------------|
| Deep-copy of large graphs is slow / stack-overflows | Iterative encode with explicit stack; document cost; reject cycles |
| `ObjFn` entry invalid if program not shared | Always install same `SharedProgram` on children |
| Deadlock: `join` from worker on parent while parent joins worker | Document; no cycle detection in v1 |
| Test flakiness / cwd locks | Prefer no cwd mutation; use absolute paths; avoid global process state |
| `Machine` fields still `!Send` after output fix | Audit `pending_ffi`, raw pointers; don’t move a Machine mid-`execute` |
| Panic in worker across `catch_unwind` | Catch unwinds at thread boundary; map to `JoinFailed` |

---

## Success criteria

- `use thread::*;` works like `use io::*;`.
- `spawn` + `join`, directional channels, and `ThreadPool` all work end-to-end in examples.
- Deep-copy sends structured values (including `Fn` captures and nested aggregates); `Stream` / `Coroutine` fail cleanly.
- No `ARCHIVE_VERSION` bump; no new opcodes.
- Coroutines and single-threaded programs unchanged.
- `cargo test --workspace` green; docs updated.
