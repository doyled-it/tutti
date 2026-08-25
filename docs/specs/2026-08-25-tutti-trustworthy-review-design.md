# Trustworthy autonomous review: verify-loop + ship-gate + no-nits reviewer

Status: proposed
Date: 2026-08-25

## Goal

Make it impossible for a green-gated bug to ship through the autonomous drain. Today the
engine runs a Reviewer, but the reviewer's judgment does not authorize the merge (CI does),
and a fix is never re-reviewed. This epic makes a clean review a hard precondition for the
ship, verifies every fix by re-reviewing it, and eliminates subjective review nits by
design rather than tolerating them.

## Motivation (evidence)

An overnight run built three tutti-design modules (E1.5 skill harness, E6 forge decomposer,
E3 page renderer) via an orchestration of implementer -> spec review -> adversarial
code-quality review -> fix -> re-review -> merge-only-when-clean. Every one of the three
code-quality rounds caught a real, shipping-severity bug behind a fully green gate:

- E1.5: a Blocking CRLF bug in the hand-rolled SKILL.md frontmatter parser that silently
  corrupted or truncated skill bodies, plus three over-strict lint rules.
- E6: a Major idempotency-marker collision (non-ASCII and punctuation-only issue titles
  collapsed to the same marker, which would silently skip genuinely-new issues as
  duplicates).
- E3: a Blocking u32 underflow panic in an SVG builder, plus a false-pass in a validator.

All three had a passing `fmt` + `clippy -D warnings` + tests gate. Tutti's drain engine, which
merges on a green gate, would have shipped all three.

## Current flow and its gaps

`engine.rs::run_one_hooked` today: Implementer -> Reviewer (fresh agent, produces a
`ReviewReport`) -> if `report.needs_fixes()`, FixApplier runs ONCE (must return
`ReadyToShip`) -> commit -> PR -> CI green -> merge (`exec.ship`).

Three gaps:

1. **No re-review after the fix.** The engine takes the FixApplier's `ReadyToShip` on faith
   and ships; it never re-runs the Reviewer to confirm the finding was actually resolved.
2. **The ship gates on CI, not on a clean review.** The reviewer's verdict only triggers one
   fix pass; the real ship gate is `exec.ship` (CI green). A reviewer-flagged, superficially
   patched change can still merge.
3. **One review role conflates spec-fit and correctness, and raises subjective nits.** A
   single Reviewer mixes "matches the plan" with "is correct," and an adversarial reviewer
   with no opinionated backstop generates endless subjective style findings that never
   converge.

## Decisions (settled)

- **A clean review is a hard ship precondition.** `exec.ship` runs only after the review
  loop converges to zero Blocking/Major findings. CI green is still required, no longer
  sufficient.
- **Verify loop.** review -> if any Blocking/Major finding, fix -> re-review -> repeat, up to
  `max_review_iterations` (default 3). On exhaustion with a surviving Blocking/Major finding,
  park the issue as `needs-human` (the existing block path). Every fix is re-reviewed.
- **Severity gating.** The loop and the ship-gate key on **Blocking + Major** findings.
  **Minor** findings are advisory notes: they never block the ship, never extend the loop,
  and are NOT auto-fixed. A review clean of Blocking/Major ships the exact tree that was
  reviewed, with no post-review mutation, so "reviewed state == shipped state" holds
  unconditionally. Minors are eliminated by design (opinionated tooling + the conventions
  doc, I2/I3), not patched at ship time. This guarantees termination (subjective minors are
  otherwise endless) and keeps the ship-gate airtight (an unreviewed edit can never ship).
- **The Reviewer is adversarial and correctness-scoped.** Its prompt is "assume the gate and
  CI are green; find the correctness bugs they cannot see," and it is explicitly told NOT to
  raise formatting, style, naming, or structure findings, those are settled by the opinionated
  gate and project convention. This is what removes the nit problem at the source, not a
  leniency knob.
- **No nits by design (opinionated engine).** Rather than tolerate subjective findings, they
  are designed out: mechanical concerns become hard gate failures (opinionated tooling), and
  subjective choices are pre-decided (a project conventions doc / constitution). This is the
  same "opinionated engine" stance tutti already takes for scaffolding.

## Architecture

- **`crates/tutti-core/src/engine.rs` (`run_one_hooked`):** replace the single review ->
  maybe-one-fix block with a bounded loop. Pseudocode:

  ```
  let mut iterations = 0;
  loop {
      let report = run_role(Reviewer, ...).await?;          // fresh agent each time
      if !report.has_blocking_or_major() { break; }          // clean -> proceed to ship
      iterations += 1;
      if iterations > cfg.max_review_iterations {
          park_for_human(...); return Blocked("review did not converge");
      }
      let fix = run_role(FixApplier, Some(report), ...).await?;
      if fix.status != ReadyToShip { park_for_human(...); return Blocked(...); }
      handoff = fix.handoff...;                               // carry forward
  }
  // ... commit, then exec.ship (CI gate) as today
  ```

- **`ReviewReport` gating predicate:** `needs_fixes()` currently keys on the verdict; add
  `has_blocking_or_major()` (any finding with `Severity::Blocking | Severity::Major`) and drive
  the loop on that, so the gate is severity-based, not verdict-based. `Severity` already exists
  on `Finding` (`message.rs`), so this is no schema change.
- **`crates/tutti-backend-claude/src/prompt.rs`:** the `Role::Reviewer` role line becomes the
  adversarial, correctness-scoped prompt described above (and the FixApplier keeps applying the
  findings it is handed).
- **`crates/tutti-core/src/config.rs`:** add `max_review_iterations: u32` (default 3), read by
  the engine.

## Decomposition (the epic's stacked issues)

- **I1. Verify-loop + ship-gate + adversarial correctness-only reviewer.** The engine loop
  change, the `has_blocking_or_major()` predicate, the config knob, the reviewer prompt, and
  hermetic tests. The load-bearing change; build first. Independently valuable (the reviewer
  already defers style to the existing fmt/clippy gate).
- **I2. Expand the opinionated gate tooling.** Strengthen the scaffold/retrofit gate profiles
  so mechanical concerns are hard gate failures rather than review findings: more opinionated
  lints and a test-presence expectation per language profile. Shrinks the residual finding
  space toward correctness-only.
- **I3. Project conventions doc (constitution).** A short standing set of pre-decided,
  opinionated choices (style, testing, structure) seeded by the `AGENTS.md` tutti already
  scaffolds, that the implementer and reviewer both defer to, so taste is never a finding.

## Testing (hermetic)

The existing engine tests already script `Reviewer`/`FixApplier` outcomes through
`FakeBackend` (see `review_requesting_changes_triggers_fix_stage` and siblings in
`engine.rs`). Extend them:

- A Reviewer that returns a Major finding, then (after a fix) a clean report, ships, and the
  Reviewer ran TWICE (re-review happened).
- A Reviewer that returns a Blocking finding on every pass parks the issue after
  `max_review_iterations` fix attempts (no ship).
- A Reviewer whose only finding is Minor ships on the first pass with NO fix stage (Minor
  does not gate and is not auto-fixed; the reviewed tree ships as-is).
- A clean first review ships with no fix stage (unchanged happy path).

## Out of scope

- Changing the CI gate itself (CI remains a required, independent gate; this adds the review
  gate in front of it).
- The two-stage spec-compliance + code-quality split. Tonight's spec-compliance passes were
  low-yield, so the single adversarial correctness reviewer is the chosen shape; a spec stage
  can be revisited if evidence warrants.
- The full content of the opinionated lints (I2) and the constitution (I3); this spec sets
  their intent, each gets its own detailed treatment.

## Open questions

- (Resolved) A Minor-only finding is not auto-fixed at all: the reviewed tree ships as-is,
  so no unreviewed edit can ever ship. Minors are eliminated by design (I2/I3), not patched.
- The exact per-language opinionated lint set for I2 (deferred to I2's own design).
