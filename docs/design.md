# `tutti design`: design on rails

`tutti design` is the front stage of the funnel. It turns a raw application idea into a
self-contained `design.html` and a reviewed milestone/epic/issue backlog that the drain engine
then executes. Where the `superpowers` brainstorming flow designs one feature at a time, `tutti
design` designs the whole application, and it branches to fit the project's shape.

It is a hybrid: the engine owns a fixed sequence of design **movements** (the rails) and their
ratification gates; within each movement an agent facilitates a real Socratic conversation, free
to go deeper on the novel or complex parts. Structure at the boundaries, freedom within.

## The movements

A session walks these movements in order. Each ends only when you ratify its section.

| # | Movement | The question it answers |
|---|----------|-------------------------|
| 0 | Constitution | What must stay true no matter what? |
| 1 | Frame | Who is this for, why do they care the day it ships, what is the budget, what is out? |
| 2 | Impact | What behavior change, in which actor, produces the goal? |
| 3 | Domain | What is the language and shape of this world, and where are the seams? |
| 4 | Decide | What are we building, and why this shape over the alternatives? |
| 5 | Structure | How do the pieces fit at each zoom level? |
| 6 | Slice | What is the thinnest end-to-end path, then the ribs? |
| 7 | Decompose | What are the small, testable, dependency-ordered units? |

Movements that earn a picture invoke a diagram sub-skill (flow, data flow, sequence,
architecture, network, story map) and embed a hand-authored inline-SVG diagram in the Sotto
house style.

### Branching by shape

The chain sizes its ceremony to the project, declared in Frame (or detected for an existing
repo):

- **Small CLI or library** collapses the middle: it skips Domain and Structure and keeps the
  ends. Over-ceremony is the failure mode for a small tool.
- **Mobile app** and **multi-service product** run the full eight movements; a multi-service
  product invests most in Domain and Structure, where a wrong seam is expensive.

## Running it (CLI)

```
tutti design [--repo <path>] [--resume] [--branch <name>] \
             [--target <owner/repo> --forge <github|gitea|gitlab> --login <login>]
```

The chat runs in your terminal: the agent asks one question at a time, you answer, and when it
has covered a movement it proposes that movement's section for you to **ratify** or **request
changes** on. Nothing advances without your ratification.

- **New project:** run in an empty or new repo. The chain asks for the project shape, runs the
  movements, then (with `--target`) proposes the backlog and, on your confirmation, seeds it
  onto the forge. A new repo is also scaffolded from the chosen stack.
- **Existing repo:** pass `--repo <path>`. The chain grounds itself in the code first: it
  detects the stack, reads the existing docs (README, AGENTS.md, `docs/`), and reads the code's
  structure and entities through codegraph. It then infers the project shape (you confirm or
  correct it), proposes the domain and structure it already sees for you to confirm, and treats
  the decisions the code has already made as settled, so it does not re-litigate them. The
  intent movements (Frame, Constitution) are still asked fresh, because the code cannot tell you
  who the project is for or why.
- **Resume:** `--resume` continues a stopped session from `.tutti/design/`. A session persists
  after every turn, so a stop mid-movement resumes exactly where it left off.
- **Branch:** `--branch <name>` snapshots the current session so you can explore an alternative.

### The handoff

When the chain completes, `tutti design` runs one decompose pass: it reads the ratified design
and proposes the backlog as a milestone/epic/issue tree with EARS acceptance criteria (`WHEN
[condition] THE SYSTEM SHALL [behavior]`). You review the rendered plan and confirm before
anything is created. With `--target`, it seeds the tree onto the forge with the right `status:*`
labels (idempotently: a re-run reconciles against existing issues rather than duplicating them),
and the drain engine can pick it up. Without `--target`, the session and `design.html` are still
produced; nothing is seeded.

## Running it (the app)

The Tauri app has a **Design** surface with the same chain: a per-movement chat pane, a live
preview of the accreting `design.html` beside it, and the backlog review/confirm step before
seeding. Open it from the sidebar with a project loaded.

## Where the state lives

- `.tutti/design/session.json` holds the session (shape, movements, ratified sections, the
  in-flight movement's conversation). It is the source of truth between turns and across a
  resume.
- `design.html` is the rendered design page, a single self-contained file (inlined CSS, a
  section per ratified movement, embedded SVG diagrams).

## Conventions and skills

The movement facilitation and the diagram authoring ship as Anthropic-format `SKILL.md` skills
under `skills/design/` (one per movement, one per diagram type), loaded and injected into the
agent's prompt. They are validated by an evaluation-driven harness (a structural lint plus eval
records). Tutti bundles its own skills and diagram vocabulary rather than depending on any
plugin being installed, because it runs on other people's machines.

## The live-tier smoke tests

The hermetic gate needs no `claude` or forge. The end-to-end checks are behind the crate's
`live` feature:

```
# drive a real claude session through the whole chain and produce a design.html:
cargo test -p tutti-cli --features live live_smoke

# seed a small backlog into a throwaway sandbox repo you can write to (via gh auth):
TUTTI_LIVE_SANDBOX=you/sandbox cargo test -p tutti-cli --features live live_seed
```

The seed test skips cleanly when `TUTTI_LIVE_SANDBOX` is unset.
