# Go conventions

## Accept interfaces, return concrete types; let the consumer define the interface it needs.

Tag: advisory

A function takes the small interface it uses and returns the concrete type it built, so
callers keep full access and the interface lives where it is consumed.

Bad: `func New() Storer` (hands back an abstraction the caller cannot extend)
Good: `func New() *Store` and `func Save(w io.Writer, s *Store) error`

## Keep interfaces small and defined at the point of use.

Tag: advisory

Define the one- or two-method interface next to the code that consumes it, not a wide
interface exported beside the implementation.

Bad: a `Repository` interface with twelve methods in the package that implements it
Good: `type reader interface { Read(id string) (Row, error) }` where it is used

## Use `defer` for cleanup immediately after acquiring a resource.

Tag: correctness

`defer` the close on the line after you open, so every return path releases it. A manual
close before each `return` leaks the moment someone adds an early return.

Bad: `f, _ := os.Open(p)` then a manual `f.Close()` before one of several returns
Good: `f, err := os.Open(p)` then `defer f.Close()`

## Avoid naked returns in anything longer than a few lines.

Tag: advisory

Named returns with a bare `return` hide what is being returned. In all but the shortest
functions, return the values explicitly.

Bad: `func f() (n int, err error) { ...; return }`
Good: `func f() (int, error) { ...; return n, nil }`

## Write table-driven tests with subtests (`t.Run`).

Tag: advisory

Drive cases from a slice of structs and run each under `t.Run` so a failure names the case
and one case does not stop the rest.

Bad: copy-pasted `if got := F(x); got != want { ... }` blocks
Good: `for _, tc := range cases { t.Run(tc.name, func(t *testing.T) { ... }) }`

## Pass `context.Context` as the first parameter for cancelable or request-scoped work; do not store it in structs.

Tag: correctness

Thread `ctx context.Context` as the first argument through cancelable and request-scoped
calls. Storing it in a struct outlives the request it belongs to.

Bad: `type Server struct { ctx context.Context }`
Good: `func (s *Server) Handle(ctx context.Context, req Req) error`

## Handle every error explicitly; wrap with `fmt.Errorf("...: %w", err)` and inspect with `errors.Is`/`errors.As`; never string-match a message; do not discard an error with `_` unless deliberate.

Tag: correctness

Check every returned error. Add context by wrapping with `%w` so callers can still
`errors.Is`/`errors.As` the cause. Never compare `err.Error()` to a string, and never assign
an error to `_` unless the discard is intentional and obvious.

Bad: `v, _ := strconv.Atoi(s)` or `if err.Error() == "not found"`
Good: `if err != nil { return fmt.Errorf("parse count: %w", err) }` then `errors.Is(err, ErrNotFound)`
