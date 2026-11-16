# Intent Studio — agent instructions

A client goal becomes a well-designed working site, then evidence-based
improvement. Rust + Axum + Tokio + PostgreSQL core; SvelteKit workspace;
a bounded, provider-neutral agent loop; preview-first scoped publishing.
Contract: the engineering guide `06_Intent_Studio.docx` (kept in the
maintainer's workspace under `docs/Project_Guides/`, not in this
repository) — when code and guide disagree, the guide wins unless a `D-NNN`
in `docs/DECISIONS.md` says otherwise. Status: ready for milestone I0; the
ship gate is blocked on pilot clients (README, *Blocked on*).

## Read first

1. `docs/END_GOAL.md` — what done means, scope, the non-negotiable
   invariants, the four required failure cases and the security cases
   (each with its test and pass condition), the ship gate.
2. `docs/ARCHITECTURE.md` — sources of truth, components, paths, the
   consistency story, layout.
3. `docs/DECISIONS.md` — prescribed directions, the decisions made before
   code (D-010–D-029) and the questions still open; record new decisions
   there.
4. `docs/ROADMAP.md` — I0–I6; each step's gate is its proof, and the gate
   passes before the next step starts. A status cell says *Done* only when
   it names the tests and the evidence path.

## Non-negotiables

1. **PostgreSQL is the only authority** — briefs, facts, candidates, check
   records, approvals, releases, leads, the outbox, budgets, evidence. The
   content-addressed store holds asset and bundle bytes; deploy targets
   serve copies. `intent-domain` holds no I/O — CI enforces it.
2. **No transaction spans an external call** — model, render worker,
   deployment adapter, notification provider. Commit intent to a job/outbox
   row, execute, store evidence, reconcile `uncertain` — never relabel it.
3. **The model proposes; the platform enforces.** Allowlisted tools; enforced
   bounds on iterations, wall time, tokens, tool output and total budget;
   revisions carry a reason and evidence; imported pages and assets are
   untrusted content, never instructions; structured output that fails
   validation is a named refusal, not a retry-forever.
4. **Approval binds the exact built bundle** (D-010). The hash is the
   bundle that was rendered and checked; promotion uploads those stored
   bytes with no rebuild. Promotion is refused for any other hash, for a
   bundle without a passing check record (D-022) for that exact hash, and
   for a bundle using a stale fact. Rollback is a new release.
5. **A lead and its outbox commit together** before success shows. One
   logical lead per `(site, submission_id)`; finite retries on the D-020
   schedule; a down provider loses no acknowledged lead and blocks no new
   submission. Abuse limits, tenant isolation, retention apply (D-019).
6. **Approved, current facts only on the page** (D-013). Every factual
   claim — testimonials, metrics, prices, guarantees, licences,
   certifications, insurance, service areas, hours, contact details, legal
   promises — references a fact record; free text is linted for unbound
   claims; a stale fact blocks promotion. Generated links point only at
   allowlisted targets (D-014). Keep asset originals and their licenses.
7. **No-change is a valid outcome.** Weak evidence → no change or a named
   hypothesis; nothing auto-publishes; traffic movement is not a causal
   conversion effect; low-volume sites report insufficient evidence.
8. **Bounded everything; evidence over claims** (workspace standard): full
   load runs are 1,000,000 units — lead submissions and beacon events
   against `intent-server` (D-025) — with results in `perf/results/`
   carrying machine + commit; misses are kept, not rerun until green. Never
   load-test a third party: published pages on the deploy target and the
   model, email and deploy providers get functional traffic only. Guide
   numbers are proposed targets, not measurements; laptop numbers are
   development observations.
9. **Egress and isolation.** Every fetch of a client-named URL goes through
   `intent-egress` (D-012); the render worker has no network beyond the
   bundle under test and no credentials (D-011); deploy tokens are
   decrypted only in `intent-publish` (D-017).

## Rust rules

- `export PATH="$(brew --prefix rustup)/bin:$PATH"` before every cargo
  command — Homebrew's cargo ignores the `rust-toolchain.toml` pin.
- Typed errors (`thiserror`) in libraries; `anyhow` only in binaries.
- No `unwrap`/`expect` on external input — every byte of model output is
  external input. Deadlines on every I/O await.
- No secrets, prompts, brief internals, approval tokens or lead PII in
  logs — the log-capture test (D-029) runs over every drill.

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

## Verification gate (CI from the first I0 commit)

`scripts/check.sh` runs what CI runs: `cargo fmt --all --check`;
`cargo clippy --workspace --all-targets --locked -- -D warnings`;
`cargo test --workspace --locked`; the architecture checks listed in
`docs/ROADMAP.md` §I0 (no I/O in `intent-domain`, provider clients only in
`intent-agent`, outbound HTTP for client URLs only in `intent-egress`,
`intent-review` independent of `intent-publish`); `svelte-check` and a
workspace build; the drills of every finished milestone. Run it before
every push; report exactly what ran and what passed.

## Working rules

- Work in small slices; each states the user outcome, the invariant, the
  failure case and the acceptance test.
- When the guide leaves something open, record it in `docs/DECISIONS.md` as
  an open question or a new D-NNN — never guess silently.
