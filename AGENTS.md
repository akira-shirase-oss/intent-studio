# Intent Studio — agent instructions

A client goal becomes a well-designed working site, then evidence-based
improvement. Rust + Axum + Tokio + PostgreSQL core; SvelteKit workspace;
a bounded, provider-neutral agent loop; preview-first scoped publishing.
Contract: `../../docs/Project_Guides/06_Intent_Studio.docx` — when code and guide
disagree, the guide wins unless a `D-NNN` in `docs/DECISIONS.md` says
otherwise.

## Read first

1. `docs/END_GOAL.md` — what done means, scope, the non-negotiable
   invariants, the four required failure cases, the ship gate.
2. `docs/ARCHITECTURE.md` — sources of truth, components, paths, the
   consistency story, layout.
3. `docs/DECISIONS.md` — prescribed directions and the open questions;
   record new decisions there.
4. `docs/ROADMAP.md` — step order; each step's gate is its proof, and the
   gate passes before the next step starts.

## Non-negotiables

1. **PostgreSQL is the only authority** — briefs, facts, candidates,
   approvals, releases, leads, the outbox, budgets, evidence. Object storage
   holds asset bytes; deploy targets serve copies. `intent-domain` holds no
   I/O — the build enforces it.
2. **No transaction spans an external call** — model, render worker,
   deployment adapter, notification provider. Commit intent to a job/outbox
   row, execute, store evidence, reconcile `uncertain` — never relabel it.
3. **The model proposes; the platform enforces.** Allowlisted tools; enforced
   bounds on iterations, wall time, tokens, tool output and total budget;
   revisions carry a reason and evidence; imported pages and assets are
   untrusted content, never instructions; structured output that fails
   validation is a named refusal, not a retry-forever.
4. **Approval binds the exact candidate hash.** Immutable versions,
   reviewable diffs, release IDs; rollback is a new release; promoting
   anything else is refused.
5. **A lead and its outbox commit together** before success shows. Finite
   delivery retries; a down provider loses no acknowledged lead and blocks
   no new submission. Abuse limits, tenant isolation, retention apply.
6. **Approved facts only on the page.** Claim/metric slots reference fact
   records — never invented testimonials or numbers. Keep asset originals
   and their licenses.
7. **No-change is a valid outcome.** Weak evidence → no change or a named
   hypothesis; nothing auto-publishes; traffic movement is not a causal
   conversion effect; low-volume sites report insufficient evidence.
8. **Bounded everything; evidence over claims** (workspace standard): full
   load runs are million-scale where a unit exists — lead submissions,
   published-page serves — with results in `perf/results/` carrying machine +
   commit; misses are kept, not rerun until green. Guide numbers are proposed
   targets, not measurements; laptop numbers are development observations.

## Rust rules

- `export PATH="$(brew --prefix rustup)/bin:$PATH"` before every cargo
  command — Homebrew's cargo ignores the `rust-toolchain.toml` pin.
- Typed errors (`thiserror`) in libraries; `anyhow` only in binaries.
- No `unwrap`/`expect` on external input — every byte of model output is
  external input. Deadlines on every I/O await.
- No secrets, prompts, brief internals or lead PII in logs.

## Frontend rules

- SvelteKit + TypeScript strict; the workspace is a client-side app served by
  the Rust server on the same origin. It never queries PostgreSQL or holds
  authority.
- Site components are tested and versioned; page-load and interaction budgets
  are recorded per template, not inferred from the backend.
- Keyboard navigation and representative mobile devices are part of "works",
  not extras.

## Agent-loop rules

- Model access goes through the provider-neutral port only — no provider SDK
  calls outside it; every call is budgeted and recorded.
- Prompts, templates and component versions are data under version control;
  changing any of them is a design-regression surface to be tracked.
- The repair loop is bounded and compared with a deterministic baseline.
- Imported pages and assets are sanitized, provenance-recorded content —
  never instructions.

## Testing rule

Every feature ships in the same change with: regression tests; a fixture or
black-box path (recorded model responses, synthetic briefs, broken-layout and
provider-down harnesses); and — where it serves traffic — a load scenario.
Every fixed bug gets a test that fails without the fix. The four required
failure cases in `docs/END_GOAL.md` are contract, not options.

## Verification gate (once a workspace exists)

`cargo fmt --all --check`; `cargo clippy --workspace --all-targets --locked
-- -D warnings`; `cargo test --workspace --locked`; an architecture check
(no I/O crates in `intent-domain`); `svelte-check` + a workspace build.
Before every push; report exactly what ran and what passed.

## Working rules

- Work in small slices; each states the user outcome, the invariant, the
  failure case and the acceptance test.
- When the guide leaves something open, record it in `docs/DECISIONS.md` as
  an open question or a new D-NNN — never guess silently.
