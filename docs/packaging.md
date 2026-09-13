# Packaging tutti (skills + codegraph)

`tutti design` (the Score design chain) needs two things at runtime that are not compiled into
the binary: the **movement and diagram skills** it injects into the agent's prompt, and,
optionally, the **codegraph** binary the existing-repo grounder queries. This document is how a
shipped tutti finds them, and how to bundle them.

## Building a distribution

```
scripts/package.sh [DEST]        # DEST defaults to dist/tutti
```

It builds `--release`, then stages beside the binary:

```
<DEST>/tutti            the release binary
<DEST>/skills/          the design skills (movements + diagrams), copied from the repo's skills/
<DEST>/codegraph        the codegraph binary, only when CODEGRAPH_BIN is set (see below)
```

Laying the skills beside the binary is what lets a shipped tutti resolve them with no
environment configured.

## How the skills are resolved

`tutti design` resolves the skills directory in this order (first hit wins):

1. `TUTTI_SKILLS_DIR` if set (an explicit override).
2. `skills/` beside the running binary (what `package.sh` produces).
3. `../../skills` relative to the crate at build time (the dev/test fallback, so a `cargo run`
   from the repo just works).

If none resolves, loading a movement skill returns a clear error rather than proceeding.

## How codegraph is resolved and bundled

codegraph enriches the existing-repo grounding (the domain entities and structure signal). It is
**MIT-licensed, so tutti is free to bundle it.** The grounder resolves the binary in this order:

1. `TUTTI_CODEGRAPH_BIN` if set (an explicit path).
2. `codegraph` beside the running binary (what `package.sh` bundles when `CODEGRAPH_BIN` is set).
3. bare `codegraph` on `PATH`.

If none is runnable, grounding **degrades gracefully**: the chain still runs on stack + docs
grounding, just without the codegraph-derived domain/structure signal. codegraph is an
enhancement, never a hard dependency.

### Bundle it, or install it

- **Bundle** (self-contained distribution): build codegraph for the target platform and point
  `package.sh` at it:

  ```
  CODEGRAPH_BIN=/path/to/codegraph scripts/package.sh
  ```

  This copies it to `<DEST>/codegraph`, where resolution step 2 finds it.

- **Install-and-run** (when a per-platform codegraph binary is not vendored): ship tutti without
  codegraph and have the operator install it (`codegraph` on `PATH`), or set
  `TUTTI_CODEGRAPH_BIN`. tutti works either way; a machine with no codegraph simply gets the
  lighter stack + docs grounding.

A per-platform automated release (building tutti and codegraph for each target and attaching the
staged distribution) is future work; `package.sh` is the single, scriptable staging step it will
call.
