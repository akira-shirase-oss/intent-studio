# Roadmap

Step order follows the contract's implementation table: brief and components,
reference-to-preview, reliable launch, improvement review — each with its
proof before the next starts. Numbers in the guide are proposed targets, not
measurements.

| Step | Builds | Proves (the gate) | Status |
|---|---|---|---|
| **I1 — brief, facts + components** (guide step 1) | Workspace scaffold; Postgres schema for briefs, facts, references, specs; the validated brief (intent, audience, offer, approved claims, brand, reference permissions); fact records with license/provenance; the typed page-tree spec + validator; the hand-built component set for the one local-service pattern; reference notes | One validated brief round-trips through the API; every invalid brief/spec is a named refusal; a claims or metric slot referencing an unapproved fact cannot validate; components render a spec deterministically at both viewports | Planned |
| **I2 — reference → preview** (guide step 2) | The provider-neutral model port; structured output → typed page tree; the bounded agent loop (iterations, wall time, tokens, tool output, budget); a reason + evidence record per revision; the isolated render worker; Playwright functional + visual checks at two sizes; the bounded repair loop; untrusted-import handling | Structured layouts render at desktop + mobile and detected defects get repaired; a model/render failure keeps the previous approved version with a bounded, explained retry; the repair loop is compared against a deterministic baseline, not rerun until it looks good | Planned |
| **I3 — reliable launch** (guide step 3) | Immutable candidates keyed by content hash; approval bound to the exact candidate; reviewable diffs + release IDs; the restricted build worker; the scoped deployment adapter: preview → promote → rollback; the lead API + inbox with lead + outbox in one transaction; finite notification retries; abuse limits, tenant isolation, retention | Preview approval works end to end; promotion refuses anything but the approved hash; the provider-down drill: a lead stays saved, retries stay finite, new submissions stay unblocked; the rollback drill runs on a real deploy | Planned |
| **I4 — improvement review** (guide step 4) | Authorized observation collection; the review job; the proposal queue — one measured change + hypothesis, or explicit no-change; separation of traffic changes from causal conversion effects; low-volume honesty | Weak-evidence input yields no-change or a named hypothesis; a low-volume site reports "insufficient evidence" rather than a fake experiment; every proposal is a queued, reviewable record — nothing auto-publishes | Planned |
| **I5 — load + ship** | The evidence plan: 50 synthetic briefs (completion rate, median human corrections, generation latency, cost); the lead API at offered load **with the notification provider down**; published-page benchmarks + per-template budgets; keyboard navigation + representative mobile devices; the three-client pilot | The ship gate — clients publish a site they accept, receive real authorized inquiries, and can say what changed and why; the four failure cases drilled; results committed with machine + commit under the workspace million-op standard | Planned |

## Later extensions (explicit contracts required)

Additional site patterns and categories, in-app content editing, source-aware
lead analytics, frequent health monitoring of published sites, the optional
Colophon content adapter. Each gets its own contract before work starts. The
guide excludes automatic conversion gains and daily redesigns outright.
