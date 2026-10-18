# CLAUDE.md

Two crates: `crates/aivyx-stencil` (pure tool-call grammar; consumers:
aivyx-pa by git rev) and `crates/toolbench` (benchmark CLI).

## Build, test, lint

    cargo test --workspace
    cargo clippy --workspace --all-targets -- -D warnings
    cargo fmt --all

## Rules

- `aivyx-stencil` output is no longer byte-identical to the extraction from
  aivyx-pa: since e939e61 the grammar's input key is `parameters` (so `name`
  comes first), and since b848525 local `$defs`/`definitions` are hoisted into
  the grammar root. aivyx-pa's frozen golden test is updated when pa bumps its
  pin. No I/O, no async, dependencies serde + serde_json only.
- `results/2026-10-18/DECISION.md` records why aivyx-engine was not built
  despite the mechanical verdict: read it before re-running the comparison.
- toolbench never executes a tool; it only scores replies.
- Snapshots under `fixtures/tools/` come from `toolbench capture`, never by
  hand. Scenarios are synthetic: no real user data, ever (public repo).
- Live runs need local model servers and are never in CI. See the spec in
  `aivyx-ecosystem/docs/superpowers/specs/2026-10-18-aivyx-toolbench-design.md`.
