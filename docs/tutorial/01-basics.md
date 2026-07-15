# Chapter 1 — Basics

This chapter introduces the core syntax of zero-script: literals, variables, functions, control flow, output, and the expression/statement model. By the end you will be able to write small programs like Fibonacci, arithmetic helpers, and FizzBuzz-style output.

Every zero-script program is a `.0s` file. The runtime looks for a top-level `main` function as the entry point:

```0s
fn main() {
    print "hello";
}
```

Run a file from the project root:

```bash
cargo run -- examples/fib.0s
```

---

## Comments

Line comments start with `//` and run to the end of the line:

```0s
// This is a comment.
let x = 5; // inline comment
```

Comments are ignored by the compiler. Use them to explain *why* something is written a certain way, not to restate what the code already says.

---

## Literals

zero-script has four primitive literal forms.

| Kind   | Examples              | Notes                                      |
|--------|-----------------------|--------------------------------------------|
| `int`  | `0`, `42`, `-7`       | Signed integers                            |
| `float`| `1.0`, `3.14`, `-0.5` | Must contain a decimal point (`1.0`, not `1`) |
| `string` | `"hello"`, `"FIZ"`  | Double-quoted; escape sequences follow C-style conventions where supported |
| `bool` | `true`, `false`       | Boolean literals                           |

```0s
fn main() {
    print "%i", 42;
    print "%f", 3.14;
    print "%s", "hello";
    print "%z", true;
}
```

---

## Variables

Bind a name to a value with `let`:

```0s
let x = 5;
let y = 10;
```

You may attach an explicit type after the name:

```0s
let x: int = 5;
let name: string = "zero-script";
```

When the type is omitted, the compiler infers it from the right-hand side (see [Chapter 2 — Types and Variables](02-types-and-variables.md)).

Each `let` creates a new binding in the current scope. Bindings are introduced at the point of the `let` statement and remain visible in enclosing blocks.

---

## Reassignment

After a variable is bound, update it with assignment (no `let` keyword):

```0s
let x = 5;
x = 20;
```

From `examples/let_test.0s`:

```0s
fn main() {
    let x = 5;
    print "%i", x;   // 5
    let y = 10;
    print "%i", y;   // 10
    x = 20;
    print "%i", x;   // 20
}
```

Expected output when run: `51020` (three integers printed back-to-back).

Assignment requires an existing binding. Assigning to an undeclared name is a compile-time error.

Compound assignment (`+=`, `-=`, `*=`, and the other arithmetic/bitwise forms) updates a binding in place and evaluates to the new value:

```0s
let x = 5;
x += 3;
print "%i", x;   // 8
```

Increment and decrement follow C-like rules: prefix forms (`++x`, `--x`) evaluate to the new value; postfix forms (`x++`, `x--`) evaluate to the old value. They work on variables, dict fields, and array elements.

```0s
let y = 0;
print "%i", y++;   // 0
print "%i", y;     // 1
let z = 0;
print "%i", ++z;   // 1
```

See `examples/operators.0s` for a broader operator demo.

---

## Functions

Define functions with `fn`. Parameter types and an optional return type are written in the signature; the body is a block:

```0s
fn add(int a, int b) -> int {
    return a + b;
}
```

- Parameters are comma-separated: `Type name`.
- Return type follows `->`. Omit it when the function returns nothing useful (implicit unit).
- Functions must be declared at the top level in a file (not nested inside other functions in current zero-script).

Call a function by name with parenthesised arguments:

```0s
add(3, 4);
```

From `examples/call_test.0s`:

```0s
fn add(int a, int b) -> int {
    return a + b;
}

fn main() {
    add(3, 4);      // result discarded
    print "done";
}
```

The call `add(3, 4)` is an **expression statement** — its return value is computed and then dropped.

---

## Return statements

Use `return expr;` to leave a function early with a value:

```0s
fn fib(int n) -> int {
    if n <= 2 {
        return 1;
    }
    return fib(n - 1) + fib(n - 2);
}
```

If execution reaches the end of a function body without hitting `return`, the function returns a default value (typically `0` for numeric contexts). Prefer explicit `return` when the result matters.

---

## Control flow

### `if`, `else if`, `else`

Conditions must be boolean expressions:

```0s
if n <= 2 {
    return 1;
}

if (n % 3) == 0 {
    print "FIZ";
} else if (n % 5) == 0 {
    print "BUZ";
} else {
    print "%i", n;
}
```

`else if` chains are parsed as nested `if`/`else` — you can stack as many branches as needed.

Parentheses around conditions are optional but often improve readability when mixing operators: `if (n % 3) == 0`.

### `while` loops

A `while` loop repeats its body while the condition is `true`:

```0s
let i = 0;
while (i < 3) {
    i = i + 1;
}
```

The condition is re-evaluated before each iteration. As with `if`, the condition must be boolean — `while 42 { ... }` is rejected by the typechecker.

---

## Blocks

A block `{ ... }` groups zero or more statements. Blocks create scope for `let` bindings declared inside them:

```0s
fn main() {
    let x = 1;
    {
        let y = 2;
        print "%i", x + y;
    }
    // y is not visible here
}
```

Function bodies, `if` branches, `while` bodies, and `defer` bodies are all blocks.

---

## `defer`

Schedule cleanup (or other exit work) with `defer`:

```0s
fn example() {
    defer {
        print "cleanup";
    }
    print "work";
}
```

A `defer` block runs when the **enclosing function** exits — whether by `return` or by falling off the end of the body. Multiple `defer` statements in one function run in **last-in, first-out (LIFO)** order: the defer written last runs first.

Use `defer` for resource teardown, logging, or paired setup/teardown logic without scattering cleanup across every `return` path.

---

## `print` and format specifiers

### Literal output

Print a string with no formatting:

```0s
print "hello";
print "FIZ";
```

### Formatted output

When the format string contains conversion specifiers, pass matching arguments after a comma:

```0s
print "%i", 42;
print "%i", x + y;
```

The compiler **type-checks** every specifier against its argument. A mismatch is a compile error, not a silent runtime bug.

| Specifier | Expected type | Typical use                          |
|-----------|---------------|--------------------------------------|
| `%i`      | `int`         | Signed decimal integer               |
| `%u`      | `int`         | Unsigned-style integer formatting    |
| `%x`      | `int`         | Hexadecimal                          |
| `%b`      | `int`         | Binary                               |
| `%p`      | `int`         | Pointer-style / address formatting   |
| `%f`      | `float`       | Floating-point                       |
| `%s`      | `string`      | String                               |
| `%z`      | `bool`        | Boolean (`true` / `false`)           |
| `%%`      | (none)        | Literal percent sign                 |

Example mixing integers from `examples/const.0s`:

```0s
fn sum(int a, int b) -> int {
    return a + b;
}

fn main() {
    print "%u", 2 + 2 + sum(2 + 2);
    print "%u", 2 + 2 + 2 + 2;
}
```

Common type errors:

```0s
print "%i", "hello";  // error: %i requires int
print "%s", 42;       // error: %s requires string
print "%f", 1;        // error: %f requires float (use 1.0)
```

---

## Expressions vs statements

Understanding the distinction keeps programs predictable.

| Concept      | Ends with `;`? | Produces a value? | Example                    |
|--------------|----------------|-------------------|----------------------------|
| Expression   | Optional       | Yes               | `2 + 2`, `fib(10)`, `x`    |
| Statement    | Usually yes    | Often no          | `let x = 5;`, `print "%i", x;` |

- **Expression statement**: an expression followed by `;`. The value is evaluated and discarded — e.g. `add(3, 4);`.
- **`let` binding**: a statement that introduces a name; not an expression (you cannot write `let y = let x = 5;`).
- **`return`**: a statement that exits the function with a value.
- **Blocks**: the last expression in a block may be used as the block's value in expression contexts; in statement-only contexts each inner line is typically a statement.

Function calls, arithmetic, and comparisons are expressions and can nest:

```0s
return fib(n - 1) + fib(n - 2);
print "%u", 2 + 2 + sum(2 + 2);
```

---

## Operator precedence (overview)

zero-script uses a Pratt parser with familiar C-like precedence. From highest to lowest (approximate):

1. Postfix: field access (`.field`), function call `()`
2. Prefix: `-`, `+`, `~`
3. Multiplicative: `*`, `/`, `%`, `**`
4. Additive: `+`, `-`
5. Shifts: `<<`, `>>`
6. Bitwise: `&`, `^`, `|`
7. Comparisons: `==`, `!=`, `<`, `<=`, `>`, `>=`
8. Logical: `&&`, `||`
9. Assignment: `=`

When in doubt, parenthesise:

```0s
((2 + 2) * 2) + -3
(2 + 2) * (2 + 2)
```

For the full precedence table and associativity rules, see [Operator reference](../reference/operators.md).

---

## Worked examples

The following examples build on each other. Read them in order, then run them locally.

### Step 1 — Fibonacci (`examples/fib.0s`)

Recursive functions, `if`, and formatted integer output:

```0s
fn fib(int n) -> int {
    if n <= 2 {
        return 1;
    }

    return fib(n - 1) + fib(n - 2);
}

fn main() {
    print "%i", fib(32);
}
```

Running this prints `2178309` (the 32nd Fibonacci number). Notice:

- Base case via early `return`.
- Recursive calls in an expression (`fib(n - 1) + fib(n - 2)`).
- `%i` matches the `int` return type.

### Step 2 — Variables and reassignment (`examples/let_test.0s`)

Multiple bindings and reassignment:

```0s
fn main() {
    let x = 5;
    print "%i", x;
    let y = 10;
    print "%i", y;
    x = 20;
    print "%i", x;
}
```

Output: `51020`.

### Step 3 — Calls and arithmetic (`examples/call_test.0s`, `examples/const.0s`)

Combine function calls with expression statements and `%u` formatting:

```0s
fn add(int a, int b) -> int {
    return a + b;
}

fn main() {
    add(3, 4);
    print "done";
}
```

And nested arithmetic with a helper:

```0s
fn sum(int a, int b) -> int {
    return a + b;
}

fn main() {
    print "%u", 2 + 2 + sum(2 + 2);
    print "%u", 2 + 2 + 2 + 2;
}
```

### Step 4 — FizzBuzz-style output (`examples/fizbuz.0s`)

Independent `if` checks (not `else if`) so multiples of both 3 and 5 print both fragments:

```0s
fn fizbuz(int n) {
    if (n % 3) == 0 {
        print "FIZ";
    }
    if (n % 5) == 0 {
        print "BUZ";
    }
}

fn main() {
    fizbuz(1);
    fizbuz(2);
    fizbuz(3);
    fizbuz(4);
    fizbuz(5);
    fizbuz(6);
    fizbuz(7);
    fizbuz(8);
    fizbuz(9);
    fizbuz(10);
    fizbuz(11);
    fizbuz(12);
    fizbuz(13);
    fizbuz(14);
    fizbuz(15);
}
```

For `n = 15`, both conditions hold, so output includes `FIZBUZ`. For `n = 3`, only `FIZ` prints.

**Stretch goal:** rewrite `main` with a `while` loop that calls `fizbuz(i)` for `i` from 1 to 15 instead of listing each call.

---

## Common pitfalls

1. **Forgetting semicolons** — Statements like `let`, `return`, and `print` need a trailing `;`.

2. **Using `let` on reassignment** — Write `x = 10;`, not `let x = 10;` again (that would shadow or error depending on scope).

3. **Non-boolean conditions** — `if 1 { ... }` and `while 1 { ... }` fail typechecking. Use comparisons: `if x > 0 { ... }`.

4. **Format specifier mismatches** — `%i` requires `int`, `%f` requires `float` (`1.0` not `1`), `%s` requires `string`. The checker catches these before run time.

5. **Float vs int literals** — `1.0` is a float; `1` is an int. Mixing them in arithmetic may require an explicit cast or a float literal where `%f` is used.

6. **Discarding return values accidentally** — `add(3, 4);` computes `7` and throws it away. Assign or print the result when you need it: `let r = add(3, 4);` or `print "%i", add(3, 4);`.

7. **`else if` vs separate `if`s** — Chained `else if` runs at most one branch. Separate `if` statements can each run (as in FizzBuzz when a number is divisible by both 3 and 5).

8. **Implicit return at end of function** — Relying on falling off the end without `return` may yield `0`. Be explicit for public APIs.

9. **Parentheses in tuples vs grouping** — `(1 + 2)` is a grouped expression; `(1, 2)` is a two-element tuple (covered in [Aggregates](../tutorial/05-aggregates.md)). A single-element tuple requires a trailing comma: `(1,)`.

---

## Exercises

1. Write `fn double(int n) -> int` and print `double(21)` from `main`.

2. Extend the Fibonacci example to print `fib(0)` through `fib(10)` on one line using a `while` loop.

3. Write a function `abs(int n) -> int` using `if`/`else` (no built-in `abs` assumed).

4. Use `defer` in a function that prints `"enter"`, does work, and relies on defer to print `"leave"`. Confirm LIFO order with two defers.

5. Fix the type errors in this snippet (there are three):
   ```0s
   fn main() {
       print "%f", 3;
       print "%s", 100;
       if 1 {
           print "always";
       }
   }
   ```

---

## See also

- [Chapter 2 — Types and Variables](02-types-and-variables.md) — annotations, inference, and type errors
- [Operator reference](../reference/operators.md) — full precedence and associativity
- [Aggregates](../tutorial/05-aggregates.md) — tuples, arrays, records (coming in the tutorial track)
- `examples/fib.0s`, `examples/let_test.0s`, `examples/fizbuz.0s` — source for the worked examples above
