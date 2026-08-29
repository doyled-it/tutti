// SPDX-License-Identifier: AGPL-3.0-or-later
//! The single source of truth for the opinionated gate's rule and flag lists, consumed by
//! both `scaffold` (which renders them) and `retrofit` (which installs them), so the two can
//! never drift.

/// Ruff lint rules every project selects (a floor, not a cap). Rendered into the scaffold's
/// `[tool.ruff.lint] select` and installed by retrofit into an existing pyproject.
pub const RUFF_LINT_SELECT: &[&str] =
    &["E", "F", "I", "UP", "B", "C4", "SIM", "PIE", "PERF", "RUF"];

/// Per-file ruff ignores for tests (B011 = `assert False`, legitimate in tests).
pub const RUFF_TEST_IGNORES: &[&str] = &["B011"];

/// The dev dependency group every Python project pins.
pub const PYTHON_DEV_GROUP: &[&str] = &["ruff", "mypy", "pytest"];

/// Clippy lints denied in the crate's `[lints.clippy]` table. `unwrap_used`/`expect_used`
/// are denied outside tests (clippy.toml re-allows them in tests); `dbg_macro` everywhere.
pub const CLIPPY_DENIES: &[&str] = &[
    "semicolon_if_nothing_returned",
    "manual_let_else",
    "explicit_iter_loop",
    "map_unwrap_or",
    "needless_pass_by_value",
    "inefficient_to_string",
    "implicit_clone",
    "dbg_macro",
    "unwrap_used",
    "expect_used",
];

/// TypeScript strict-family compiler flags, all enforced true (set even over a weaker value).
/// NOTE: `noPropertyAccessFromIndexSignature` is deliberately absent (it breaks
/// `process.env.FOO` access and was dropped in I2).
pub const TS_STRICT_FLAGS: &[&str] = &[
    "strict",
    "noUncheckedIndexedAccess",
    "exactOptionalPropertyTypes",
    "noImplicitOverride",
    "noFallthroughCasesInSwitch",
    "noImplicitReturns",
    "verbatimModuleSyntax",
    "forceConsistentCasingInFileNames",
];

/// The npm `check` script every TypeScript project runs (mirrors scripts/check.sh).
pub const TS_CHECK_SCRIPT: &str = "tsc --noEmit && biome check . && bun test";

/// TypeScript dev dependencies (name, semver range).
pub const TS_DEV_DEPS: &[(&str, &str)] = &[
    ("typescript", "^5.7.0"),
    ("bun-types", "^1.1.0"),
    ("@biomejs/biome", "^2.0.0"),
];

/// clippy.toml keys that re-allow unwrap/expect inside tests.
pub const CLIPPY_TEST_ALLOWS: &[&str] = &["allow-unwrap-in-tests", "allow-expect-in-tests"];
