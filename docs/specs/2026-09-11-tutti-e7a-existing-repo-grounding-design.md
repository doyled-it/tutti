# Existing-repo design grounding (E7a)

Status: proposed
Date: 2026-09-11

## Goal

Let the Score design chain read an existing codebase and ground the conversation in what the
code already is, so a design session on an existing repo confirms and sharpens rather than
asking everything from scratch. Reading a codebase well is not a thin extension of the
greenfield chain (the epic spec, `2026-08-24-tutti-design-chain-design.md`, flags this as
deserving its own design), so this spec settles: what the chain infers versus asks, how it
decides `ProjectShape`, and how it avoids re-litigating decisions the code already encodes.

## Decisions (settled)

- **Hybrid: infer the code-derivable, ask the intent.** Code reveals *what exists* (the stack,
  the structure, the domain vocabulary, decisions already encoded) but never *why* (the
  customer, the appetite, the principles). So:
  - **Infer-and-confirm** the code-derivable movements: **Domain** (glossary, entities, seams),
    the stack, and **Structure** (modules/containers). The chain presents what it sees and the
    human corrects or ratifies it.
  - **Ask-fresh-but-grounded** the intent movements: **Frame** (customer, problem, appetite,
    non-goals) and **Constitution** (principles, non-negotiables). The agent asks the normal
    Socratic questions, but with the repo context in its prompt so the questions are sharper and
    it does not ask what the code plainly answers.
- **Grounding sources (first cut): stack detection, existing docs, and codegraph structure.**
  - **Stack:** `retrofit::detect_languages` + the detected `StackProfile`.
  - **Docs:** `README`, `AGENTS.md`, and `docs/` (existing prior intent and decisions).
  - **Codegraph:** its non-interactive JSON CLI (no MCP server needed) supplies the Domain and
    Structure signal. `codegraph query <term> --kind <k> --json` lists symbols (types/entities),
    `codegraph files --json` and `codegraph status --json` give the module/file structure and
    counts. The grounder ensures the index first (`codegraph init` when `.codegraph/` is absent,
    reusing the `context::CodeGraph::ensure_ready` pattern) then reads those commands. It is one
    `RepoGrounder` impl behind the seam, so a machine without codegraph degrades to the
    stack+docs grounding rather than failing.
  - **Git history is deliberately excluded** from the first cut (noisy signal, low marginal
    value over the above).
- **Codegraph is MIT-licensed, so Tutti bundles it.** MIT permits redistribution with
  attribution, so the decision is to ship the `codegraph` binary in Tutti's distribution rather
  than requiring a separate install. At runtime the grounder uses a bundled binary if present and
  otherwise falls back to a `codegraph` on `PATH` (the existing seam already probes
  `codegraph --version`), so a dev checkout with codegraph installed and a shipped release both
  work. The release-packaging mechanics (which artifact ships the binary, per-platform) are a
  distribution task carried out where Tutti builds its release, tracked with E7/packaging; the
  bundling policy itself is settled here.
- **`ProjectShape` is detected and confirmed, not asked.** Inferred from the stack plus repo
  size/structure heuristics (a single CLI/library crate -> `SmallCli`; multiple services /
  containers -> `MultiService`; a mobile stack -> `Mobile`) and presented in Frame for the human
  to confirm or override, rather than asked cold.
- **Re-litigation guard.** The grounder extracts an `already_decided` set (facts the code has
  settled, e.g. "persistence is Postgres", "transport is iroh"). Those ride into the Decide and
  Structure movements as settled context, and the movement prompt tells the agent not to reopen
  them unless the human explicitly flags a change. This is the "do not re-litigate what the code
  already made" rule made concrete.
- **The grounder is a consumer-owned seam**, mirroring E2's `Facilitator`: `tutti-design` defines
  a `RepoGrounder` trait producing a `RepoGrounding`, with a fake for hermetic tests. The real
  implementation reads the three sources and is wired at the surface (E7).

## Architecture

### The grounding seam and type (tutti-design)

```rust
/// A read-only summary of what an existing repo already is, fed into the grounded movements.
pub struct RepoGrounding {
    /// Detected stack ids (retrofit::detect_languages), e.g. ["rust"].
    pub stack: Vec<String>,
    /// The shape inferred from the stack + structure, presented in Frame for confirmation.
    pub inferred_shape: ProjectShape,
    /// A short prose digest of the existing docs (README/AGENTS.md/docs), or empty.
    pub docs_digest: String,
    /// Domain signal from codegraph: candidate entities/nouns and module seams.
    pub domain: DomainSignal,      // { entities: Vec<String>, seams: Vec<String> }
    /// Structure signal from codegraph: the containers/modules the code is organized into.
    pub structure: Vec<String>,
    /// Decisions the code has already made, injected as settled into Decide/Structure.
    pub already_decided: Vec<String>,
}

/// The read-a-repo seam. Consumer-owned so tutti-design stays hermetic; the real impl
/// (detect_languages + a docs reader + codegraph) is wired at the surface (E7).
pub trait RepoGrounder {
    fn ground(&self, repo_root: &Path) -> Result<RepoGrounding>;
}
```

### How grounding reaches the conversation

- A grounded session captures the `RepoGrounding` once (persisted in `SessionState`, so a
  resumed session keeps it) and injects the relevant slice into each movement's prompt:
  - **Frame:** "This is an existing {stack} project; its shape looks like {inferred_shape}.
    Confirm or correct the shape, then ask the framing questions." (Intent still asked.)
  - **Constitution:** the docs digest as context ("the repo's docs suggest these principles;
    confirm or revise"); principles still asked, not asserted.
  - **Domain:** the `domain` signal as a proposed glossary/entity list to confirm or correct.
  - **Decide / Structure:** the `already_decided` set and `structure` as settled context, with
    the do-not-relitigate instruction.
- Mechanically, this extends E2's `build_turn_prompt` with an optional grounding context block,
  and `SessionState` gains `grounding: Option<RepoGrounding>`. The facilitation loop is unchanged
  otherwise; grounding is additive context, never a new control path.

### Shape inference

`infer_shape(grounding_inputs) -> ProjectShape`: a pure function over the detected stack and a
few structure heuristics (crate/package count, presence of multiple service entry points, a
mobile toolchain marker). Presented in Frame for confirmation; the human's choice wins.

## Decomposition (this issue vs E7)

- **In E7a:**
  - The `RepoGrounder` seam and `RepoGrounding`/`DomainSignal` types in `tutti-design`, with a
    fake grounder and hermetic tests.
  - Grounding injection into the movement prompts + `SessionState.grounding`, tested over the
    fake.
  - `infer_shape` (pure) + its tests.
  - A **real grounder** in `tutti-app-core` covering all three sources: stack (`detect_languages`),
    docs (filesystem read of README/AGENTS.md/`docs/`), and **codegraph** (ensure the index, then
    parse `codegraph query/files/status --json`). It degrades to stack+docs when codegraph is
    absent. Live-tested against a fixture repo (the codegraph path behind `#[ignore]`, needing the
    binary, matching the crate's live-tier convention).
- **Deferred to E7 (surface wiring + release packaging):**
  - Wiring the grounder into `tutti design --repo <path>` and the Tauri surface.
  - The per-platform release-packaging that actually ships the codegraph binary in the Tutti
    distribution (the bundling policy is settled here: MIT permits it, so bundle with a `PATH`
    fallback; only the release-artifact mechanics ride with E7/packaging).

## Testing (hermetic)

- `infer_shape`: golden cases (single Rust crate -> SmallCli; multi-service -> MultiService; a
  mobile marker -> Mobile).
- Grounding injection: with a fake `RepoGrounder` returning a known `RepoGrounding`, the Domain
  movement prompt contains the proposed entities, the Decide prompt contains the
  `already_decided` set plus the do-not-relitigate instruction, and Frame contains the inferred
  shape for confirmation. Frame/Constitution prompts still ask (they do not assert the intent).
- The real grounder (tutti-app-core): a live-tier test over a small fixture repo asserts the
  stack and docs digest are populated; a separate `#[ignore]`d codegraph test (needs the binary)
  asserts the domain/structure signal is populated from `codegraph *--json` on a fixture.

## Out of scope

- Git-history grounding (excluded from the first cut).
- The per-platform release packaging that ships the codegraph binary (E7); the bundle decision
  itself is settled here.
- Any change to the greenfield movement selection or the facilitation loop's control flow
  (grounding is additive context only).
- Regenerating grounding mid-session as the repo changes (grounding is captured once per session;
  a re-ground is a new session).

## Open questions

- The exact structure heuristics in `infer_shape` (crate count vs entry-point detection); the
  first cut can be coarse and lean on the human confirmation in Frame.
- Whether the docs digest is raw excerpts or an agent-summarized paragraph (first cut: raw
  excerpts capped by length; a summarization pass can come later).
