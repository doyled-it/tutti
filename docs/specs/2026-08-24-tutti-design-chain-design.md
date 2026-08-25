# Tutti design chain ("Score"): app-level design on rails

Status: proposed
Date: 2026-08-24
Working name: **Score** (the full written score the agent-ensemble then performs; provisional). The rail units are **movements**.

## Goal

Give Tutti a front stage: a railed, agent-facilitated design conversation that turns a
raw application idea into (a) a self-contained, Sotto-style HTML design page and (b) the
milestone/epic/issue backlog the drain engine executes. Today Tutti can impose an
agent-friendly shape (scaffold), harden an existing repo (retrofit), and drain a
`status:ready` backlog. What it cannot do is produce that backlog in the first place. That
work, the application-level design thinking, is still a human sitting with an agent for a
long conversation. Sotto's entire design page and its whole issue tree came out of exactly
such a conversation. This feature makes that a first-class Tutti capability.

Superpowers `brainstorming` does this at the **issue** level (one feature at a time). This
does it at the **whole-application** level, and it **branches** to fit the project. It is
design on rails, feeding implementation on rails.

## Scope

This spec is the **epic-level design** for the whole pipeline, end to end, for both new
and existing repos, on both the CLI and the Tauri app. It is decomposed into stacked
sub-issues (see "Decomposition"). Each sub-issue gets its own detailed spec and plan as it
is picked up; this document is the coherent whole they slot into, so the seam between "the
thinking chain" and "the backlog it feeds" is designed once, not twice.

## Decisions (settled)

- **Facilitation is hybrid: design on rails.** The engine owns a canonical sequence of
  design **movements** (the rails), their gates, and the branching. Each movement is a real
  Socratic mini-conversation the agent facilitates, with freedom to linger and go deeper on
  the novel/complex/interesting parts rather than marching lockstep. Structure at the
  movement boundaries, freedom within a movement.
- **The artifact is a self-contained HTML/CSS/JS design page** in the Sotto house style,
  with **hand-authored inline-SVG diagrams**, not Mermaid. (Sotto's `docs/index.html` is a
  single self-contained file whose diagrams are inline SVG built from a small shared CSS
  vocabulary: `.diagram`, `.svg-node`, `.nodetext`, `.nodesub`, `.flowlbl`, and edges.
  Those diagrams read better than Mermaid, so Tutti ports that vocabulary and targets it.)
- **Tutti-native and self-contained.** Tutti ships its own design-rails prompt library and
  diagram vocabulary. It does not depend on the superpowers plugin being installed, because
  Tutti runs on other people's machines.
- **Skills are first-class loadable files, not inline prompts.** The movement facilitation
  guidance and the diagram authoring ship as **Anthropic-format `SKILL.md` skills**
  (agentskills.io open standard), loaded through the `ClaudeBackend`'s existing
  `/plugin:skill` path, and proven by an **evaluation-driven test harness**. See the Skills
  section. This is a dedicated sub-issue (E1.5) that lands before the facilitation loop
  (E2) consumes the seam.
- **State lives under `.tutti/design/`** so a session is stoppable, resumable, and
  **branchable** (fork a session to explore an alternative decision).
- **Placement: a new front stage of the funnel**, exposed on both the CLI (`tutti design`)
  and the Tauri app, covering **new and existing** repos.
- **Movement 7 seeds the forge via propose -> review/edit -> seed.** The chain proposes the
  full backlog as a reviewable plan; the human ratifies (cut, reorder, edit acceptance
  criteria); only then does Tutti create it on the forge. This matches the existing
  retrofit `plan -> y-confirm -> apply` pattern and guards against a decomposition that
  silently drops scope (a documented failure mode of PRD-to-tasks parsers).
- **Artifacts are gates, not paperwork.** No movement advances without the human ratifying
  its section. This is the universal insight across every design methodology surveyed.

## Research basis

The movement sequence is synthesized from established practice (sources are illustrative,
not exhaustive):

- **Amazon Working Backwards / PR-FAQ** (write the ending first; the strongest cheap
  forcing function for "brainstorm fully before building").
- **Basecamp Shape Up** (a fixed *appetite* instead of an estimate; explicit No-Gos and
  rabbit holes).
- **Impact Mapping** (Adzic) and **User Story Mapping** (Patton) for tying deliverables to
  a goal and for slicing a walking skeleton, which becomes the milestone/epic backbone.
- **Domain-Driven Design + Event Storming** (Brandolini) for discovering bounded contexts
  and a ubiquitous glossary (full event-storm only at large scope).
- **Google/Oxide RFD design-doc culture** and **Nygard ADRs** for the decision record
  (alternatives considered, consequences, immutable-and-superseded).
- **C4 model** (Brown) for progressive structural decomposition (context, container,
  component).
- **AI-native spec pipelines**: AWS Kiro (requirements/design/tasks, EARS acceptance
  criteria, each gated), GitHub spec-kit (specify/plan/tasks plus a standing
  `constitution.md`), PRD-to-tasks agents (dependency-aware task graphs, complexity flags).

## The rails (movements)

| # | Movement | Guiding question | Produces | Diagram skills |
|---|---------|------------------|----------|----------------|
| 0 | **Constitution** | What must stay true no matter what? | Project principles / non-negotiables (carried in every later movement's context; feeds `AGENTS.md`) | — |
| 1 | **Frame** | Who is this for, why do they care the day it ships, what is the budget, what is explicitly out? | PR-FAQ / pitch + appetite + non-goals | — |
| 2 | **Impact** | What behavior change, in which actor, produces the goal? | Impact map (goal -> actors -> impacts -> deliverables) | flow |
| 3 | **Domain** | What is the language and shape of this world, and where are the seams? | Glossary + entities/events -> bounded contexts (full event-storm only at large scope) | data/event flow |
| 4 | **Decide** | What are we building, and why this shape over the alternatives? | Design doc core: alternatives considered, chosen approach, risks, open questions | data-transport / sequence |
| 5 | **Structure** | How do the pieces fit at each zoom level? | C4 (context -> container -> component) + ADRs for irreversible choices | architecture, network/deployment |
| 6 | **Slice** | What is the thinnest end-to-end path, then the ribs? | Story map / walking skeleton -> milestone+epic skeleton | user-story map |
| 7 | **Decompose** | What are the small, testable, dependency-ordered units? | requirements/design/tasks with EARS criteria + dependency & complexity metadata -> **the forge issue tree** | — |

### Branching

Two axes:

1. **By project shape** (declared in Frame, or detected for existing repos via retrofit's
   language detection + codegraph):
   - **CLI / library (small):** collapse the middle. Skip Domain and Structure, one ADR at
     most, a flat task list. Over-ceremony is the failure mode here.
   - **Mobile app (medium):** keep Frame, Impact, Slice, Decompose plus a platform-
     constraints note (offline, permissions, store review); go light on Domain and
     Structure (one client + a backend container).
   - **Multi-service product (large):** run all eight. Domain (event storming to bounded
     contexts) and Structure (C4 + a running ADR series) are load-bearing because a wrong
     seam is expensive. Decide becomes a numbered RFD series; Decompose emits milestones and
     epics, not a flat list.
   - **General rule:** size the ceremony to the reversibility and blast radius of the
     decisions, not to the line count. Small projects compress the middle and keep the ends;
     large projects invest in the middle where integration risk lives.
2. **By decision:** a chosen alternative at a movement opens or closes downstream questions,
   and can fork the session to explore an alternative in parallel.

## Skills: movement facilitation and diagram authoring

The rails content and the diagram authoring ship as **loadable skills in the Anthropic
`SKILL.md` open standard** (agentskills.io), the format the `ClaudeBackend` already loads
via `/plugin:skill`. Skills are therefore first-class: individually authored, versioned,
user-overridable, and testable in isolation, rather than prompt strings compiled into the
binary. Each movement's facilitation guidance is a skill; each diagram type (flow,
architecture, sequence/data-transport, user-story map, network/deployment) is a skill.

### Format (per Anthropic's authoring guidance)

- A skill is a directory with a `SKILL.md` (YAML frontmatter + markdown body) plus optional
  reference files (one level deep) and a `scripts/` directory.
- Required frontmatter is `name` (<= 64 chars, lowercase/numbers/hyphens, no reserved
  words) and `description` (<= 1024 chars, third person, stating both what it does AND when
  to use it, with trigger keywords). Progressive disclosure: name + description is the
  always-loaded surface; the body (<= 500 lines) loads on trigger; references and scripts
  load only when needed.
- One default approach with an escape hatch, concrete input/output examples over abstract
  prose, consistent terminology, no time-sensitive phrasing, forward-slash paths.
- A diagram skill carries the shared Sotto inline-SVG vocabulary as a reference file and a
  `scripts/` validator.

### Test harness (evaluation-driven)

Anthropic's guidance is to build evaluations before the prose and ships no runner, so Tutti
provides one. Each skill ships at least three evaluation records of the shape
`{ skills, query, inputs, expected_behavior[] }`, where `expected_behavior` is a rubric of
observable outcomes. The harness has three layers:

1. **Structural lint (deterministic):** frontmatter present and within limits, name and
   description rules, body <= 500 lines, references one level deep, forward-slash paths, no
   reserved words. A pure check, hermetic.
2. **Diagram validators:** for a diagram skill, the produced inline SVG is well-formed and
   renders (shared with E4's validators).
3. **Behavioral evals:** run the skill over its scenarios against a no-skill baseline and
   score the `expected_behavior` rubric. Live-tier (needs a real backend).

Sources: Anthropic Agent Skills best practices
(https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices), the
"Equipping agents for the real world with Agent Skills" engineering post
(https://www.anthropic.com/engineering/equipping-agents-for-the-real-world-with-agent-skills),
and the open standard (https://agentskills.io).

## Architecture

A new **`tutti-design`** crate, the rails engine, reusing Tutti's existing seams:

- `AgentBackend` (drive `claude -p` for facilitation and per-movement generative work),
- `Forge` (movement 7 seeding: milestones/epics/issues),
- config and events.

It sits in front of the existing scaffold (new repo) and retrofit (existing repo) paths.

### Components

1. **Rails model.** Movements are declarative data (same philosophy as `StackProfile`):
   each movement is `{ id, guiding questions, must-hit coverage checklist, output artifact
   section, applicable diagram skills, ratification gate }`. Branching is which movements are
   selected and at what depth, driven by `ProjectShape`. Adding or reshaping a movement is
   editing data, unit-testable against golden output.
2. **ProjectShape.** `small_cli | mobile | multi_service` (extensible). Declared during
   Frame or detected for existing repos. Selects the rail subset and each movement's depth.
3. **Session state machine.** Drives movement by movement: for each movement, run a
   facilitated mini-conversation (checklist ensures coverage, agent has freedom for depth),
   then gate. State persists to `.tutti/design/` (`session.json` plus the accreting
   artifacts). Resumable and branchable.
4. **Facilitation prompt library.** Tutti-native, self-contained per-movement Socratic
   prompts (the rails content): one question at a time, aware of when a diagram earns its
   place, carrying the constitution to prevent drift.
5. **Diagram sub-skills.** Generate inline SVG in the ported Sotto house style
   (`svg-node`/`nodetext`/`nodesub`/`flowlbl`/edge vocabulary): architecture (C4-ish),
   flow, sequence/data-transport, user-story map, network/deployment. Each is a prompt plus
   the shared vocabulary plus a well-formedness/renders validator.
6. **Artifact renderer.** Accretes movement outputs into one self-contained `design.html`
   (brand palette, a section per movement, embedded SVG, appended ADRs, the story map, and
   the proposed backlog), plus a machine-readable sidecar backlog manifest.
7. **Forge decomposer.** Story map + design -> milestones/epics/issues with EARS acceptance
   criteria (`WHEN [condition] THE SYSTEM SHALL [behavior]`) and dependency/complexity
   metadata. Renders a reviewable plan (in the page and as a printable diff); on confirm,
   seeds via the `Forge` seam with the right `status:*` labels. Idempotent re-runs (match
   existing issues by marker, as the GitHub forge already does for stale-PR recovery).
8. **Surfaces.** CLI `tutti design [--repo owner/name | path] [--resume] [--branch <name>]`;
   Tauri "Design" surface (a per-movement chat pane, a live preview of the accreting design
   page, and the backlog review/confirm UI).

### Data flow

```
raw idea
  -> ProjectShape (declared or detected)
  -> select rails
  -> for each movement:
       facilitate (agent + human)
       -> capture artifact section (+ diagrams)
       -> ratify gate
  -> accrete into design.html + .tutti/design/ state
  -> movement 7 proposes backlog plan
  -> human review / edit
  -> seed forge (milestones / epics / issues, status labels)
  -> hand off: scaffold (new repo) or drain (existing repo)
```

### Integration with existing Tutti

- **New repo:** design runs first. Its `ProjectShape` and stack recommendation feed the
  existing scaffold (`seed_stack` / `StackProfile`) and `InitParams`; then movement 7 seeds
  the backlog; then the drain engine runs.
- **Existing repo:** design runs alongside or after retrofit, grounding itself in the
  repo via retrofit's language detection and codegraph, and seeds a "what should this
  become / what is next" backlog onto the hardened repo.

  **This grounding warrants its own detailed spec** (tracked as **E7a**, a prerequisite of
  E7). Reading an existing codebase well is not a thin extension of the greenfield chain:
  it has to infer the current shape (from codegraph, existing docs, the retrofit's detected
  stack, and git history), decide how much to present back for the human to confirm versus
  ask fresh, reconcile the constitution and Frame against what the code already implies, and
  avoid re-litigating decisions the code has already made. That deserves designing
  deliberately rather than being folded into the CLI wiring issue.

## Error handling and gates

- Every movement ends on explicit human ratification before advancing.
- Resumable at any movement; a session can be forked to explore an alternative.
- The constitution rides in every movement's context to prevent drift over a long chain.
- Backlog seeding is pre-flighted (dry-run/diff) and idempotent, so a re-run reconciles
  against existing issues rather than duplicating them.

## Testing (Tutti's tiers)

- **Hermetic:** rails state machine, `ProjectShape` branching selection, artifact
  accretion, SVG diagram validators, the forge decomposer, and the HTML renderer, all over
  in-memory fakes (`AgentBackend` + `Forge`), matching `tutti-core`'s style.
- **Live tier** (`--features live`, `#[ignore]`): a real `claude -p` facilitation smoke, a
  real seed into `tutti-live-sandbox`, and "design.html opens and renders." "Diagrams look
  good" stays a manual check.

## Decomposition (stacked sub-issues)

- **E1. Crate skeleton.** `tutti-design` with the rails model, `ProjectShape`, the session
  state machine, and in-memory fakes. Hermetic core.
- **E1.5. Skill seam + test harness.** The loadable-skill packaging (Anthropic `SKILL.md`
  format, `ClaudeBackend` `/plugin:skill` wiring) and the evaluation-driven test harness
  (structural lint + the eval-record runner + the diagram-validator hook). Lands before E2,
  the first consumer, so facilitation and E4/E5 author skills against a proven seam.
  Grounded in Anthropic's skill-authoring guidance (see the Skills section).
- **E2. Single-movement facilitation loop.** Drive one movement end to end over the fake
  `AgentBackend`: checklist coverage plus free depth, ratification gate, state persist. The
  movement's guidance is loaded as a `SKILL.md` skill via E1.5's seam.
- **E3. Design-page renderer + SVG vocabulary.** Port the Sotto inline-SVG diagram
  vocabulary into a reusable asset; render the accreting self-contained `design.html`.
- **E4. Diagram sub-skills.** The five generators (architecture, flow, sequence/data-
  transport, user-story map, network/deployment) authored as `SKILL.md` skills against
  E1.5's harness, each with its inline-SVG validator and eval records.
- **E5. Full rails + branching.** The eight movement facilitation skills (`SKILL.md`,
  authored against E1.5's harness) and the branching rule set (small / mobile /
  multi-service), with golden tests.
- **E6. Forge decomposer.** Story map -> milestones/epics/issues + EARS + dependency/
  complexity; plan/diff/confirm/seed via the `Forge` seam; idempotent.
- **E7a. Existing-repo design grounding (own spec).** How the chain reads an existing
  codebase (codegraph, existing docs, retrofit's detected stack, git history), how much it
  infers versus asks, and how it reconciles the constitution/Frame against what the code
  already implies. Prerequisite of E7's existing-repo path.
- **E7. CLI `tutti design`.** New and existing repo, resume/branch, plus wiring into
  init/scaffold and retrofit.
- **E8. Tauri Design surface.** Per-movement chat, live design-page preview, backlog
  review/confirm.
- **E9. Live smokes + docs.** Live-tier tests and user-facing documentation.

## Out of scope (for now)

- Non-Claude agent backends for facilitation (the `AgentBackend` seam allows them; not a
  first target).
- Multi-user / collaborative design sessions.
- Automatic re-generation of the design page from a mutated backlog (the page is the
  source; the backlog is derived, not the reverse).
- Importing an existing external design doc as the starting point (a possible later entry,
  analogous to Kiro's design-first mode).

## Open questions

- Final top-level name ("Score" vs "Design" vs another musical term). The rail units are
  settled as **movements**.
- Whether the constitution should write through to `AGENTS.md` immediately or only on
  handoff to scaffold.
- Exact `.tutti/design/` on-disk schema for a branched session (fork semantics).
