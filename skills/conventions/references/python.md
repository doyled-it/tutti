# Python conventions

## Use `pathlib.Path` over `os.path` string juggling.

Tag: advisory

Build and join paths with `Path`, which handles separators and gives you `.exists()`,
`.read_text()`, and `/` joining.

Bad: `os.path.join(os.path.dirname(f), "out.txt")`
Good: `Path(f).parent / "out.txt"`

## Use `dataclasses` for structured records rather than ad-hoc dicts or tuples.

Tag: advisory

A record with named, typed fields is a `@dataclass`, not a `dict` the reader has to guess
the keys of or a positional tuple.

Bad: `p = {"x": 1, "y": 2}` then `p["x"]`
Good: `@dataclass\nclass Point:\n    x: int\n    y: int`

## Use f-strings for formatting.

Tag: advisory

Interpolate with an f-string, not `%`, `.format()`, or `+` concatenation.

Bad: `"hi " + name` or `"hi %s" % name`
Good: `f"hi {name}"`

## Manage resources (files, locks, sessions) with `with` context managers.

Tag: correctness

A file, lock, or session opened outside a `with` leaks on an early return or an exception.
Bind it to a `with` block so it is released on every path.

Bad: `f = open(p); data = f.read()` (never closed on an exception)
Good: `with open(p) as f:\n    data = f.read()`

## Type-hint public function signatures (mypy strict makes these load-bearing).

Tag: advisory

Annotate parameters and return types on public functions; under `mypy --strict` an
unannotated signature is a checker gap, not a shortcut.

Bad: `def total(items):`
Good: `def total(items: list[int]) -> int:`

## Iterate directly and use comprehensions, `enumerate`, and `zip` rather than C-style index loops.

Tag: advisory

Iterate the object itself. Reach for `enumerate` when you need the index and `zip` when you
walk two sequences together.

Bad: `for i in range(len(xs)): x = xs[i]`
Good: `for i, x in enumerate(xs):`

## Prefer EAFP (try/except) over precondition-checking; do not use a bare `except`.

Tag: correctness

Attempt the operation and catch the specific error rather than racing a precondition check.
Never write a bare `except:`; it swallows `KeyboardInterrupt` and `SystemExit` and hides
real failures.

Bad: `try:\n    ...\nexcept:\n    pass`
Good: `try:\n    return cache[key]\nexcept KeyError:\n    return compute(key)`
