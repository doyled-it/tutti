# TypeScript conventions

## Model variant data as discriminated unions (a shared literal `kind` field) so the compiler narrows each case.

Tag: advisory

Give each variant a shared literal tag so the compiler narrows the union in a `switch` or
`if`, instead of a bag of optional fields that are all `T | undefined`.

Bad: `type S = { kind: string; a?: number; b?: number }`
Good: `type S = { kind: "a"; a: number } | { kind: "b"; b: number }`

## Make exhaustive `switch`es provably complete with a `never`-typed default, so a new variant fails compilation.

Tag: correctness

Assign the scrutinee to a `never` in the `default` arm. When someone adds a variant and
forgets a case, the assignment fails to compile rather than silently falling through.

Bad: `default: return 0;`
Good: `default: { const _x: never = s; return _x; }`

## Use `readonly` and `as const` for data that should not be mutated.

Tag: advisory

Mark fields and arrays `readonly`, and freeze literal data with `as const`, so an accidental
write is a type error.

Bad: `const DAYS = ["mon", "tue"]` (type `string[]`, mutable)
Good: `const DAYS = ["mon", "tue"] as const`

## Prefer `interface` or `type` aliases for public object shapes over inline anonymous types.

Tag: advisory

Name a public object shape so signatures read cleanly and the shape has one definition to
change.

Bad: `function draw(p: { x: number; y: number }): void`
Good: `interface Point { x: number; y: number }` then `function draw(p: Point): void`

## Prefer type guards over assertions (`as`); assert only when you genuinely know more than the compiler.

Tag: correctness

A `value as T` is an unchecked promise the compiler cannot verify. Narrow with a type guard
so a wrong shape is caught, not asserted away.

Bad: `const u = data as User`
Good: `if (isUser(data)) { /* data is User here */ }`

## Avoid `any`; take `unknown` at untyped boundaries and narrow before use; do not silence errors with `// @ts-ignore` or `!` where narrowing would do.

Tag: correctness

`any` disables checking for everything it touches. Accept `unknown` at a boundary (parsed
JSON, a caught error) and narrow it. Do not paper over a real type error with `// @ts-ignore`
or a `!` non-null assertion when a check would prove it.

Bad: `function parse(s: string): any` then `JSON.parse(s)!.id`
Good: `function parse(s: string): unknown` then narrow before reading `.id`
