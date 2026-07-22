# New-project wizard (guided `tutti.toml` initialization)

Status: designed

## Problem

Picking a folder with no `tutti.toml` currently reveals an inline form in the left
sidebar: five unlabeled text boxes and a select, crammed into a 160px rail. Nothing on
screen says what "staging" or "true" mean, which values are legal, or what happens if you
get one wrong. It is unusable by anyone who has not read `config.rs`.

It is also incomplete. `Config` has more settings than the form exposes, so the form
silently hardcodes trunk, routing, `max_issues_per_run`, `require_label`, and
`skip_labels` to their defaults with no way to see or change them.

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
│ New Tutti project                Step 3 of 10│  header: title + counter + progress dots
├──────────────────────────────────────────────┤
│ Which repository?                            │  question heading
│                                              │
│ The repo Tutti reads issues from and opens   │  description paragraph
│ pull requests against. Detected from your    │
│ git remote.                                  │
│                                              │
│ [ doyled-it/oxidra                         ] │  control
│                                              │
│ Example: doyled-it/oxidra                    │  example line
│ Detected from your git remote.               │  contextual note
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

| # | Field(s) | Control | Description shown | Example / default |
|---|---|---|---|---|
| 1 | `dir` | read-only path + **Choose a different folder...** | "The local git checkout Tutti will work in. It must already be a git repo with your project in it." | shows what the probe detected |
| 2 | `forge_kind`, `login` | 3 radio cards | GitHub: "Issues and PRs on github.com or GitHub Enterprise. Requires the `gh` CLI, already logged in." GitLab: "Issues, merge requests and epics on gitlab.com or self-hosted. Requires `glab`, already logged in." Gitea: "Issues and PRs on Gitea, Forgejo or Codeberg. Requires `tea`, already logged in." Choosing Gitea reveals a required **login** text field: "Which `tea` login to use. This is the name you gave the host when you ran `tea login add`. Run `tea login list` to see yours." | login example: `codeberg` |
| 3 | `repo` | text | "The repo Tutti reads issues from and opens pull requests against." Note when pre-filled: "Detected from your git remote." | GitHub/Gitea: `doyled-it/oxidra`. GitLab: `group/subgroup/project` |
| 4 | `trunk` | text | "Your protected branch. Tutti never merges into it and never commits to it directly. Promoting work from the integration branch to trunk stays a human decision." | default `main` |
| 5 | `routing`, `integration_branch` | 2 radio cards | Trunk (recommended): "Every issue branches off one integration branch and merges back into it. Simple, and what you want unless you are running phased milestones." Phase stacking: "Each milestone gets its own integration branch stacked on the previous one, so phase N builds on phase N-1 before any of it reaches trunk." Trunk reveals **integration branch**: "The branch Tutti merges finished work into. It must exist, and must not be your trunk." | default `staging` |
| 6 | `model` | select of known ids + **Custom...** revealing a text field | "Which model the coding agent runs as. Sonnet is the balanced default; Opus is stronger and slower on hard work; Haiku is fastest and cheapest for mechanical tasks." | default `claude-sonnet-5` |
| 7 | `gate_commands` | repeatable rows with add/remove | "Commands that must pass before Tutti will ship an issue's work. They run in order in your repo root; the first non-zero exit fails the gate and the work goes back for a fix. Leave the single `true` if you do not want a gate yet." | examples `cargo test`, `npm test`, `uv run pytest` |
| 8 | `require_label`, `skip_labels` | text + chip list with add/remove | "Tutti only picks up issues carrying the required label, and never picks up one carrying a skip label." Callout: "Tutti will create `status:ready`, `status:in-progress` and `status:done` in your forge if they do not exist yet, and will move each issue between them as it works." | defaults `status:ready`, `status:needs-human` |
| 9 | `max_issues_per_run` | number | "How many issues one Run will work through before stopping. A safety ceiling, not a target: the run also stops when nothing is ready." Note: "Tutti always merges with a merge commit, never a squash or rebase." | default `25` |
| 10 | (review) | read-only `<pre>` of the rendered file | "This is exactly what will be written to `tutti.toml`. Nothing has been created yet." | primary button reads **Create project** |

Step 8's callout is a statement of existing behavior: `init_project` already calls
`seed_status_labels`. The wizard just makes it visible before the fact.

## Validation

Client-side, mirroring `Config::validate` so a failure surfaces on the step that caused
it rather than as a backend error string after Create:

| Step | Rule | Message |
|---|---|---|
| 2 | Gitea requires a non-empty login | "Gitea needs a `tea` login. Run `tea login list` to see yours." |
| 3 | non-empty, at least one `/`, no leading/trailing slash, no whitespace | "Enter it as `owner/repo`." |
| 4 | non-empty, no whitespace | "Enter a branch name." |
| 5 | integration branch non-empty when routing is `trunk`; must not equal trunk | "The integration branch must be different from your trunk branch." |
| 6 | non-empty when Custom | "Enter a model id." |
| 7 | at least one row, no blank rows | "Add at least one command, or use `true` for no gate." |
| 8 | `require_label` non-empty; no blank skip chips | "Enter the label Tutti should require." |
| 9 | integer >= 1 | "Enter a number of 1 or more." |

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
