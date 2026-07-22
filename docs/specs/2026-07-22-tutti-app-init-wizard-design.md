# New-project wizard (guided `tutti.toml` initialization)

Status: implemented

## Problem

Picking a folder with no `tutti.toml` currently reveals an inline form in the left
sidebar: five unlabeled text boxes and a select, crammed into a 160px rail. Nothing on
screen says what "staging" or "true" mean, which values are legal, or what happens if you
get one wrong. It is unusable by anyone who has not read `config.rs`.

It also asks the wrong questions. The settings that matter at setup (trunk, routing) are
not exposed at all, while the ones it does ask for are either already knowable from the
git remote or not answerable this early.

## Goal

Replace the inline form with a modal wizard that walks through every question one at a
time, and for each one explains what the choice does, offers the legal options where the
answer is an enum, and shows a concrete example. The wizard is the only path to creating
a `tutti.toml` from the app.

## User flow

1. Sidebar → **+ Add project** → **Choose folder...**
2. `probe_project` runs as it does today.
   - Has a `tutti.toml`: unchanged, the project is added immediately.
   - No `tutti.toml`: the **wizard modal opens**, pre-filled from the probe.
3. The user steps through the questions, then reviews the rendered file and creates.

Cancel at any point closes the modal and writes nothing.

## The modal

A centered overlay, `min(640px, 90vw)` wide, `max-height: 85vh` with the question body
scrolling. Backdrop is a dimmed scrim; clicking it or pressing Escape cancels.

Layout:

```
┌──────────────────────────────────────────────┐
│ New Tutti project                Step 2 of 5 │  header: title + counter + progress dots
├──────────────────────────────────────────────┤
│ What is your trunk branch?                   │  question heading
│                                              │
│ Your protected branch. Tutti never merges    │  description paragraph
│ into it and never commits to it directly.    │
│                                              │
│ [ main                                     ] │  control
│                                              │
│ Default: main                                │  default hint
├──────────────────────────────────────────────┤
│ [Cancel]                    [Back]  [Next →] │  footer
└──────────────────────────────────────────────┘
```

Interaction rules:

- **Enter** advances (or submits on the last step). **Escape** cancels.
- **Next** is disabled when the current step is invalid, with the reason rendered
  inline beneath the control. Validation is per-step, so an error can never be
  scrolled past.
- **Back** never loses an answer; all state lives in one object for the wizard's life.
- Focus moves to the step's primary control on every step change.
- Progress dots are indicators only, not clickable (keeps the "answer in order" model,
  since later steps' help text depends on earlier answers, e.g. the repo example depends
  on the forge).

### Question anatomy

Every step renders through one `QuestionCard` shell so they stay visually identical:
heading, description paragraph, control slot, then optional **Example:** and
**Default:** lines. Radio-card steps render each option as a bordered card with a bold
label and a one-line description, selected state using `--accent-border` / `--accent-bg`.

## The steps

The wizard does not ask for anything it can already read. `probe_project` gives the repo
slug and the forge kind from the folder's git remote, so on a normal clone both of those
questions disappear and a GitHub project is five steps, four of which have a default you
can accept.

The visible steps are therefore derived (`stepsFor`), not a fixed list:

| Step | Shown when |
|---|---|
| `folder` | always |
| `forge` | the remote did not identify one, **or** the forge is Gitea (which needs a `tea` login no URL can supply) |
| `repo` | the remote did not identify one |
| `trunk` | always |
| `routing` | always |
| `model` | always |
| `review` | always |

| Step | Field(s) | Control | Description shown | Example / default |
|---|---|---|---|---|
| `folder` | `dir` | read-only path + **Choose a different folder...** | "The local git checkout Tutti will work in. It must already be a git repo with your project in it." | reports what the probe detected, or that it detected nothing |
| `forge` | `forge_kind`, `login` | 3 radio cards | GitHub: "Issues and pull requests on github.com or GitHub Enterprise. Requires the `gh` CLI, already logged in." GitLab: "Issues, merge requests and epics on gitlab.com or a self-hosted instance. Requires `glab`." Gitea: "Issues and pull requests on Gitea, Forgejo or Codeberg. Requires `tea`." Gitea reveals a required **login**: "The name you gave the host when you ran `tea login add`. Run `tea login list` to see yours." | login example: `codeberg` |
| `repo` | `repo` | text | "The repo Tutti reads issues from and opens pull requests against." | GitHub/Gitea: `doyled-it/oxidra`. GitLab: `group/subgroup/project` |
| `trunk` | `trunk` | text | "Your protected branch. Tutti never merges into it and never commits to it directly. Promoting finished work from the integration branch up to trunk stays a human decision." | default `main` |
| `routing` | `routing`, `integration_branch` | 2 radio cards | Trunk (recommended): "Every issue branches off one integration branch and merges back into it. Simple, and what you want unless you are running phased milestones." Phase stacking: "Each milestone gets its own integration branch, stacked on the previous one, so phase N builds on phase N-1 before any of it reaches trunk." Trunk reveals **integration branch**, which must exist and must not equal trunk. | default `staging` |
| `model` | `model` | select of known ids + **Custom...** | "Sonnet is the balanced default. Opus is stronger and slower on hard work. Haiku is fastest and cheapest for mechanical tasks." | default `claude-sonnet-5` |
| `review` | (none) | read-only `<pre>` of the rendered file | "This is exactly what will be written to tutti.toml. Nothing has been created yet." Two callouts: the labels Tutti will create, and that the gate is the no-op. | primary button reads **Create project** |

### What the wizard deliberately does not ask

Each of these is seeded in `wizard.ts` and stated on the review step rather than asked
about, because none of them is a decision the user is equipped to make at setup time:

- **`require_label` / `skip_labels`.** A fixed convention (`status:ready`,
  `status:needs-human`). The engine, the board columns, and label seeding all have to
  agree on these names. Letting setup diverge from the convention buys nothing but a
  class of confusing mismatches, so `REQUIRE_LABEL` and `SKIP_LABELS` are constants.
- **`max_issues_per_run`.** It bounds a single drain, and the app's run driver loops
  drain until you pause it, so as a setup question it is noise. Seeded to
  `MAX_ISSUES_PER_RUN` (1,000,000), with `render_tutti_toml` emitting a comment above
  the key so a large number in the file does not read as a typo.
- **`gate_commands`.** What must pass before Tutti ships is not a setup fact: it falls
  out of the brainstorming conversation about what the project is and how it gets
  verified. Asking here forces a guess at the moment the user knows least, and the guess
  then sticks in the config. Seeded to `NO_OP_GATE` (`true`), the review step says so,
  and setting a real gate belongs to the orchestrator conversation (issue #15).
  `toInitForm` falls back to it rather than ever emitting an empty command list, which
  would mean the same thing while being much easier to misread in the file.
- **`ci_max_polls`, `poll_delay_secs`, `merge_mode`, `roles`.** Tuning knobs. Hand-edit
  `tutti.toml` to change them.

## Validation

Client-side, mirroring `Config::validate` so a failure surfaces on the step that caused
it rather than as a backend error string after Create:

| Step | Rule | Message |
|---|---|---|
| `forge` | Gitea requires a non-empty login | "Gitea needs a `tea` login. Run `tea login list` to see yours." |
| `repo` | non-empty, at least one `/`, no leading/trailing slash, no whitespace | "Enter it as `owner/repo`." |
| `trunk` | non-empty, no whitespace | "Enter a branch name." |
| `routing` | integration branch non-empty when routing is `trunk`; must not equal trunk | "The integration branch must be different from your trunk branch." |
| `model` | non-empty | "Enter a model id." |

Because steps can be hidden, per-step validation is not enough on its own: detection can
hand back a repo slug the `repo` step would have rejected, and that step is skipped
*because* detection succeeded. `validateAll` re-checks every rule (hidden steps included)
before Create writes anything, and reports the failure on the review step.

Validation lives in a pure module so it is testable without mounting anything.

## Code structure

**Frontend** (`tutti-app/src/lib/`):

- `wizard.ts` (new, pure): the `WizardState` type, `initialState(probe)`, `STEP_COUNT`,
  and `validateStep(state, index): string | null` returning the error message or null.
  No Svelte, no Tauri: unit-tested with vitest.
- `components/InitWizard.svelte` (new): the modal. Owns `WizardState`, renders the
  current step, wires Back/Next/Cancel/Create, fetches the preview for step 10, and
  calls `onInit(form)`.
- `components/QuestionCard.svelte` (new): the shared question shell (heading,
  description, slot, example, default).
- `components/Sidebar.svelte` (modified): drops all the inline init form markup, state
  and styles. On a probe with no config it now sets `wizardProbe` and lets the page
  render the modal. Everything else in the sidebar is untouched.
- `routes/+page.svelte` (modified): renders `<InitWizard>` at shell level when a probe
  is pending, so the modal overlays the whole window rather than living inside the rail.
- `ipc.ts` (modified): widened `InitForm`, plus `previewTuttiToml(form)`.

**Backend:**

- `commands.rs`: `InitForm` gains `trunk`, `routing`, `max_issues_per_run`,
  `require_label`, `skip_labels: Vec<String>`, and `gate_commands: Vec<String>`
  (replacing the singular `gate_command`). A private `params_from(form) -> InitParams`
  is shared by `init_project` and the new `preview_tutti_toml` command so the preview
  and the write can never diverge.
- `preview_tutti_toml(form) -> String`: pure, no filesystem, no run guard.
- `tutti-app-core`: no new types. `InitParams` and `render_tutti_toml` already cover
  every field; the wizard just stops hardcoding five of them.

Note that `Config` also has `ci_max_polls`, `poll_delay_secs`, `merge_mode` and `roles`.
These stay at their defaults and are deliberately not asked: they are tuning knobs, not
setup decisions, and asking about them would bury the questions that matter. Editing
`tutti.toml` by hand remains the way to change them.

## Testing

- `wizard.test.ts`: `initialState` respects the probe (repo and forge kind pre-filled,
  falling back to `github` and empty), and one case per validation rule above, both the
  failing and passing side.
- `commands.rs`: a test that `params_from` maps every form field onto `InitParams`.
- `tutti-app-core`: extend the existing round-trip test so a non-default value for each
  newly exposed field (trunk, routing, max_issues_per_run, require_label, skip_labels,
  multiple gate commands) survives `render_tutti_toml` → `Config::load`.

Manual: initialize a real repo through the wizard, confirm the step-10 preview matches
the file on disk byte for byte, and confirm the status labels appear in the forge.

## Out of scope

- Editing an existing project's config (the wizard only creates).
- Creating the repo, the integration branch, or the milestone in the forge.
- Validating that the repo or the integration branch actually exists before Create; that
  failure still surfaces from `activate`, which already cleans up the orphan file.
