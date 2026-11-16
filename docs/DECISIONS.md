# Decisions

Deliberate choices, newest last. D-NNN numbering follows the workspace
convention; these seed the contract's prescribed directions. Where the guide
leaves a choice open, it is an open question below — not a silent guess.

- **D-001 — Rust + Axum + PostgreSQL, not the existing prototype.** The guide
  prescribes Rust + Axum + Tokio + PostgreSQL for briefs, jobs, sites,
  approvals, leads and evidence, and explicitly does not assume the prototype
  is production-ready. The cost is re-implementing a working demo; the gain
  is one typed authority with enforceable bounds instead of carryover risk.
- **D-002 — A typed page tree, never free HTML.** The model emits a
  structured spec over tested components; the renderer is deterministic;
  claim and metric slots reference approved fact records, so a spec asserting
  an unapproved testimonial or number fails validation. This trades
  generative freedom for validation, reviewable diffs and a guarantee that
  the site cannot invent praise.
- **D-003 — Approval binds the candidate hash.** Site versions are immutable
  with release IDs; promoting anything but the approved hash is refused, and
  rollback is a new release pointing at a prior version. The cost is real —
  no quick post-approval tweak without regenerating and re-approving; the
  gain is an audit trail, safe rollback and no ambiguity about what is live.
- **D-004 — The agent loop is bounded, and the bounds live in Postgres.**
  Iterations, wall time, tokens, tool output and total budget are enforced
  limits; every revision carries a reason and its evidence; tool output is
  bounded; imported pages are untrusted content. The cost: some runs die
  incomplete (a named, retryable state, not a silent hang). The gain: a
  stop-loss on cost and a replayable record of why each change was made.
- **D-005 — The lead and its notification outbox commit in one transaction.**
  Success is shown only after both commit; delivery is finite retries against
  a provider that may be down. The cost: the outbox + notifier machinery for
  what could be a fire-and-forget email. The gain: the guide's headline
  failure case — no acknowledged lead is ever lost, and a slow provider never
  blocks a new submission.
- **D-006 — Preview first; promotion needs scoped permission + rollback.**
  Deployment goes through scoped adapters (GitHub/Vercel or alternative), not
  ambient credentials. The cost is adapter complexity and scoped-token
  management; the gain is least-privilege blast radius and a rehearsed way
  back.
- **D-007 — Builds run in a restricted worker.** Rendering and builds execute
  isolated from the API process and its credentials because they run
  generated code over untrusted imports. The cost is orchestration; the gain
  is containment.
- **D-008 — No-change is a first-class review outcome.** The weekly review
  may recommend nothing; weak evidence produces a named hypothesis or
  no-change, never an auto-published optimization. The cost is a quieter
  changelog; the gain is honesty about what the evidence supports — and the
  guide's fourth failure case stays passable.
- **D-009 — Depth over breadth for the pilot.** One local-service category,
  one quote-request pattern, three reachable clients. The cost is a product
  that looks small; the gain is tested failure behavior instead of a gallery
  of untested templates.

## Open questions the guide leaves to implementation

- **Deployment adapter.** "Scoped GitHub/Vercel or alternative" — the first
  adapter is undecided; pick one and keep the port narrow so a second is
  additive.
- **Object storage backing.** Authorized assets need a store; local
  filesystem versus S3-compatible is undecided.
- **Actor and tenant model.** Tenant isolation is required for leads, but the
  guide does not define who signs in (operator, client, both) or what exactly
  a tenant is.
- **Worker isolation mechanism.** Process, container or VM for the restricted
  builder — the contract requires isolation, the mechanism is open.
- **"Authorized observations."** The collection mechanism and consent surface
  for improvement evidence are unspecified.
- **Lead retention and qualification.** Retention is required and the outcome
  is "qualified quote requests", but durations and qualification rules are
  open; abuse limits are mandatory, their shapes open.
- **Health monitoring.** Named "frequent" under the full direction without
  targets or cadence.
- **Model providers and credential custody.** Provider-neutral is required;
  the concrete provider set and where keys live are open.
