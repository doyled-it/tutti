# Spike: activating a named skill in `claude -p` (headless)

Date: 2026-07-11
For: Plan B, the `ClaudeBackend` adapter.

## Question

How does a headless `claude -p` run load and follow a specific named skill (e.g.
`superpowers:test-driven-development`), so the backend can drive a role's skills?

## Finding (recommended mechanism)

Skills auto-load in `-p` mode by default (personal, project, bundled, and plugin
skills like `superpowers:*`). To force a SPECIFIC named skill, put its slash-command
form in the prompt text. No CLI flag is required; the prompt parser expands it.

```bash
claude -p "/superpowers:test-driven-development

<the task text>" \
  --output-format stream-json \
  --allowedTools "Read,Edit,Bash" \
  --permission-mode auto
```

- Plugin skills use the namespaced form `/plugin:name` (e.g.
  `/superpowers:requesting-code-review`).
- Personal/project skills need no namespace (e.g. `/android-pro ...`).
- Multiple skills: reference several in one prompt (textual order, no hard ordering
  guarantee), or across turns with `--resume <session_id>`.

So the `ClaudeBackend` builds each prompt as: the role's skill slash-commands (from
`RolePlaybook.skills`, converting `superpowers:x` to `/superpowers:x`), then the issue
context, then the fixed handoff-protocol postamble. It spawns
`claude -p <prompt> --model <m> --output-format stream-json` with `current_dir` set to
the worktree.

## Caveats

- `--bare` disables skill auto-discovery; do NOT pass it.
- Skills are not tools: `--allowedTools` governs the tools a skill uses (Read/Edit/Bash),
  not skill activation. The backend runs with permissions that let the skill act
  (`--permission-mode` / `--allowedTools`, or the `--dangerously-skip-permissions`
  path the SOTTO engine used for full autonomy; decide per risk).
- The exact flag surface (`--output-format`, `--permission-mode`, `--allowedTools`,
  `--resume`) should be re-confirmed against the installed `claude --help` when Plan B
  builds the spawn call, since the CLI surface changes across versions. Treat the
  slash-command-in-prompt mechanism as the stable core.

## Fallback (if slash activation ever proves unreliable)

Read the skill's `SKILL.md` markdown and inject it as a prompt preamble (the slice-1
s2.1 "preamble fallback"). On-disk locations verified on this machine:

- Personal skills: `~/.claude/skills/<name>/SKILL.md`
- Project skills: `<repo>/.claude/skills/<name>/SKILL.md`
- Plugin skills (superpowers): `~/.claude/plugins/cache/superpowers-marketplace/superpowers/<version>/skills/<name>/SKILL.md`

## Decision for Plan B

Primary: slash-command-in-prompt. Keep the resolver that maps a skill ref
(`plugin:name`) to `/plugin:name` behind a small function so swapping to the
markdown-preamble fallback is a one-line change if a live integration test shows the
slash form is not reliably honored.
