# Conventions skill: a per-language convention contract for the implementer and reviewer

Status: proposed
Date: 2026-09-10

## Goal

Make convention adherence a systematic, testable procedure both agent roles share, rather
than prose the reviewer skims. A single `conventions` skill (Anthropic SKILL.md shape, with
per-language reference files) is injected into the implementer's and reviewer's prompts. The
implementer writes convention-adherent code from it; the reviewer checks against it and
classifies each convention issue by a severity tag the skill carries. The skill's convention
list is the single source of truth from which the scaffolded `AGENTS.md` idiom bullets derive,
so the two cannot drift.

## Motivation

Epic #61 gave the engine a trustworthy review: an adversarial, correctness-scoped reviewer
(I1), an opinionated mechanical gate (I2), and an `AGENTS.md` constitution the roles defer to
(I3). But conventions today are passive: they live as prose in `AGENTS.md` and as one sentence
in the reviewer prompt telling it to defer. Nothing makes convention-checking directive,
per-language, or testable. The E1.5 skill harness (`crates/tutti-design`: a SKILL.md loader,
a structural lint, and eval records with a proxy scorer) was built for exactly this shape but
has never been used, and no SKILL.md file exists in the repo yet.

This makes conventions active on both sides of the loop: the implementer has a per-language
authoring guide, the reviewer has a per-language checklist with a built-in severity policy, and
the E1.5 harness validates and evaluates the skill. It composes with I1 rather than reversing
it: I1 told the reviewer "do not raise style, defer to AGENTS.md", and this replaces that vague
deferral with a precise contract, which conventions are correctness-bearing (the reviewer does
raise them, as gating findings) and which are advisory (a Minor note that never gates).

## Decisions (settled)

- **One skill, per-language reference files (progressive disclosure).** A single
  `conventions` skill whose SKILL.md body is the shared authoring-and-review contract, with
  `references/{rust,python,typescript,go}.md` loaded only for the task's language. This is
  Anthropic's recommended shape and uses the harness's `Skill.references` field as designed.
- **Both roles, one skill.** The skill is injected for the Implementer and FixApplier (to
  write adherent code) and the Reviewer (to check it). One artifact serves writing and review.
- **The skill tags each convention's severity.** Every convention entry in a reference file
  carries a tag: `correctness` (a violation is a real defect the mechanical gate cannot catch,
  e.g. Go not passing `context.Context` so cancellation silently breaks, or a Rust non-exhaustive
  match that lets a new variant compile) or `advisory` (a genuine-but-subjective preference). The
  reviewer maps `correctness` to a Major finding (which gates the ship under I1) and `advisory`
  to a Minor note (which never gates and is never auto-fixed, per I1). This is what makes the
  reviewer side load-bearing without reintroducing the nit-loop I1 was built to kill.
- **The skill is canonical; `AGENTS.md` derives.** The per-language convention list, the idiom
  one-liner plus its severity tag, lives in one machine-readable const spine co-located with the
  skill and drift-locked to the reference prose. The scaffold's `AGENTS.md` idiom bullets are
  emitted from that spine (a refactor of I3's inline `ConstitutionParts.idioms` arrays), and a
  drift-guard test asserts every spine idiom appears verbatim as a heading in the matching
  reference file with a matching tag. One edit point, three anchored consumers (skill prose,
  const spine, `AGENTS.md`), no silent drift (the lesson from I2a #67).
- **Delivery is preamble injection via the E1.5 loader, not slash-command auto-discovery.**
  The engine loads the skill from its in-repo directory with the E1.5 `Skill` loader, selects
  the reference(s) for the language(s) it detects in the worktree, and injects the SKILL.md body
  plus those reference(s) into the role's prompt. This is deterministic and self-contained (no
  dependency on a plugin being installed in the spawned agent's environment), it is the skill
  activation spike's documented fallback (`docs/notes/2026-07-11-skill-activation-spike.md`),
  and it is the reason the harness is a programmatic loader rather than relying on Claude Code's
  auto-discovery. The existing slash-command wiring (`tutti:planning`, `superpowers:*` via
  `default_roles`) is unchanged and untouched.
- **Evals are authored now, live judging deferred.** Each language ships eval records (a
  planted convention violation the skill should catch, plus a clean case it should not flag). The
  E1.5 structural lint and proxy scorer run them in the hermetic gate now; the live behavioral
  judge (a real backend judged against `expected_behavior`) rides the harness's already-deferred
  live-eval increment, unchanged by this work.

## Architecture

### The skill directory (in-repo, tutti-owned)

```
skills/conventions/
  SKILL.md              # name + description frontmatter; body = the shared contract
  references/
    rust.md  python.md  typescript.md  go.md
  evals.json            # EvalRecord[] (planted violation -> expected detection)
```

The SKILL.md body states the contract both roles read: for the implementer, "write code that
satisfies every convention below"; for the reviewer, "for each convention, a violation tagged
`correctness` is a Major finding, one tagged `advisory` is a Minor note; do not invent
conventions beyond this list". Each reference file lists conventions as entries of the same
shape: the idiom (one line, matching the spine verbatim), a short good/bad example, the severity
tag, and what to look for.

### The const spine (single source of truth)

A module (extending the `baseline.rs` pattern from I2a, e.g. `conventions.rs` in
`crates/tutti-app-core`) holds the machine-readable convention list per language:

```rust
pub struct Convention {
    pub idiom: &'static str,      // the one-liner, verbatim in the reference file heading
    pub severity: ConventionSeverity,  // Correctness | Advisory
}
pub const RUST_CONVENTIONS: &[Convention] = &[ /* ... */ ];
// PYTHON_CONVENTIONS, TYPESCRIPT_CONVENTIONS, GO_CONVENTIONS
```

`scaffold.rs`'s `ConstitutionParts.idioms` is refactored to read the idiom strings from this
spine (I3's inline arrays move here). A drift-guard test asserts, per language, that each
spine idiom appears as a heading in the matching reference file and that the tags agree.

### Engine wiring (preamble injection)

- The conventions skill is added to `default_roles()` for `Implementer`, `FixApplier`, and
  `Reviewer` (as a skill ref the engine resolves to a loaded skill, not a slash command).
- At prompt-build time, the engine detects the worktree's language(s) with the existing
  `retrofit::detect_languages` (or the repo's `StackProfile` when known), loads the conventions
  skill via the E1.5 `Skill` loader, selects the matching reference file(s), and injects the
  SKILL.md body plus those reference(s) as a prompt preamble for the role. A polyglot worktree
  injects every detected language's reference.
- The Reviewer `role_line` in `prompt.rs` is updated: the current "do NOT raise formatting,
  style, naming, or structural findings; those are settled by AGENTS.md" clause becomes a
  pointer to the injected skill's severity tags (raise `correctness` conventions as Major,
  `advisory` as Minor notes, nothing beyond the list). No `Finding`/`Severity` schema change:
  the existing I1 gate already keys on Blocking/Major, so a `correctness` convention violation
  gates through the mechanism already in place.

### Data flow

```
issue -> engine detects worktree language(s)
      -> loads skills/conventions via E1.5 Skill loader
      -> selects references/<lang>.md for each detected language
      -> injects SKILL.md body + reference(s) into the role prompt
Implementer/FixApplier: writes code adherent to the injected conventions
Reviewer: emits findings, tagging convention violations Major (correctness) or Minor (advisory)
          -> I1 gate ships only when clean of Blocking/Major
```

## Decomposition (the epic's stacked issues)

- **C1. Skill mechanism + Rust end-to-end.** The `skills/conventions/` directory (SKILL.md +
  `references/rust.md` + `evals.json` for Rust), the `conventions.rs` const spine with
  `RUST_CONVENTIONS`, the `scaffold.rs` refactor so the Rust `AGENTS.md` idioms derive from the
  spine, the drift-guard test, the engine wiring (detect language, load skill, inject preamble
  for the three roles), and the reviewer `role_line` update. Proves the whole mechanism on one
  language. Build first.
- **C2. Python.** `references/python.md`, `PYTHON_CONVENTIONS`, the Python `AGENTS.md` derive,
  drift guard, evals. Follows C1's pattern.
- **C3. TypeScript.** As C2, for TypeScript.
- **C4. Go.** As C2, for Go.
- **C5. Polyglot injection + reviewer-contract finalization.** Inject every detected language's
  reference in a polyglot worktree; an end-to-end hermetic test that a planted `correctness`
  convention violation is surfaced as Major (and gates) while an `advisory` one is Minor (and
  does not); and the live-eval record wiring note (the judge itself stays deferred).

## Testing (hermetic)

- The E1.5 structural lint passes on the conventions skill (frontmatter present, body within the
  line budget, references and evals well-formed).
- Per language, the drift-guard test: every spine idiom is a heading in the reference file, tags
  agree, and the emitted `AGENTS.md` idiom bullets match the spine.
- Per language, the eval records load and proxy-score (the E1.5 scorer) against the skill.
- The reviewer-contract test (C5): a fixture review over a planted `correctness` violation yields
  a Major finding (gates), a planted `advisory` violation yields a Minor note (does not gate),
  driving the existing `FakeBackend` review path.

## Out of scope

- The live behavioral eval judge (a real backend judged against `expected_behavior`): it rides
  the harness's already-deferred live-eval increment, unchanged.
- A distributable tutti Claude Code plugin (`/tutti:conventions` via a shipped marketplace):
  preamble injection makes it unnecessary now; packaging the skill for external reuse is a later
  note, not part of this epic.
- Writing the skill into the target worktree's `.claude/skills/` (rejected: it would appear in
  the built branch's diff).
- Any change to the mechanical gate (I2) or to what `AGENTS.md` says beyond sourcing its idiom
  bullets from the spine (I3).
- The Score design-chain skills (E4 diagram sub-skills, E5 movement skills): those use the same
  E1.5 harness but are their own open work under epic #44.
