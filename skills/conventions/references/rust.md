# Rust conventions

## Make illegal states unrepresentable: model variants as enums and match them exhaustively.

Tag: correctness

Model mutually-exclusive states as an `enum` and `match` all arms, so a new variant fails
compilation rather than slipping through a stringly-typed check.

Bad: `struct S { kind: String, a: Option<i32>, b: Option<i32> }`
Good: `enum S { A(i32), B(i32) }` with an exhaustive `match`.

## Prefer `?` for error propagation; reserve `unwrap`/`expect` for tests and provable invariants, and give `expect` a message.

Tag: correctness

Propagate with `?`. In non-test code an `unwrap`/`expect` is a latent panic; if it is truly
unreachable, `expect("why it cannot fail")`.

## Do not panic across a public API; return a `Result` instead.

Tag: correctness

A public function that can fail returns `Result<_, E>`; it does not `panic!`, `unwrap`, or
index-out-of-bounds on caller input.

## Use newtypes over stringly-typed or primitive-obsessed APIs, so the representation can change without breaking callers.

Tag: advisory

Wrap a domain value in a `struct Name(String)` rather than passing bare `String`/`i64`.

## Accept borrowed or generic arguments (`&str`, `&[T]`, `impl AsRef<_>`) and return owned values (`String`, `Vec<T>`).

Tag: advisory

Take `&str`/`&[T]`; return `String`/`Vec<T>`. Do not require an owned argument you only read.

## Derive standard traits (`Debug`, `Clone`, `PartialEq`) rather than hand-implementing; derive `Debug` on all public types.

Tag: advisory

`#[derive(Debug, Clone, PartialEq)]` on data types; hand-implement only when the derive is wrong.
