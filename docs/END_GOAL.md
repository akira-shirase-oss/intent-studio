# Intent Studio — end goal

**A client brief becomes a designed, tested, published website — and
evidence, not optimism, decides what changes next.**

An operator serves a service business that wants a concrete outcome, such as
qualified quote requests. The system records the client's intent, audience,
offer, approved claims, brand and reference permissions; generates a typed
page tree from tested components; renders and checks it at desktop and mobile
sizes; and presents a strong first preview — the client sees the candidate,
not the agent's internal iteration. Approval binds to the exact immutable
candidate; a scoped deployment adapter publishes a preview first, then a
versioned promotion with a rollback path. Leads land in an inbox that
survives a dead notification provider. Improvement is a review, not an
autopilot: weak weekly evidence correctly recommends no change.

Contract: `../../docs/Project_Guides/06_Intent_Studio.docx`. Per the guide, the
existing prototype is not assumed production-ready — the Rust backend is the
new proposal, not a port of working code.

## In scope (the first useful release)

- One local-service category, one quote-request site pattern — piloted with
  three reachable clients, not many untested templates.
- Brief + facts intake: intent, audience, offer, approved claims, brand,
  reference permissions; asset originals kept with license/provenance.
- Generation: provider-neutral model calls → typed page tree → render at two
  viewports → Playwright functional + visual checks → bounded repair loop.
- Approval bound to the exact candidate hash; immutable site versions,
  reviewable diffs, release IDs.
- Build in a restricted worker; preview deploy; promotion under scoped
  permission; rollback as a new release.
- Lead capture API + inbox: the lead and its notification outbox commit in
  one transaction before success is shown; finite retries; abuse limits;
  tenant isolation; retention.
- Improvement review: authorized observations → one measured proposal or an
  explicit no-change; proposals are queued for review, never auto-published.
- The SvelteKit workspace: brief, candidate review, approval, lead inbox,
  evidence. No native shell.

## Out of scope (each needs an explicit contract)

- Additional site patterns and categories, in-app content editing,
  source-aware lead analytics, frequent health monitoring of published sites.
- The optional Colophon content adapter.
- Automatic conversion gains and daily redesigns — the guide excludes both;
  they are never promised.
- Causal experiment claims on low-volume sites — declared unsupported, not
  faked.

## Non-negotiable invariants

1. **PostgreSQL is the only authority** for briefs, facts, design specs,
   candidates, approvals, releases, leads, the outbox, budgets and evidence.
   Object storage holds asset bytes; Postgres holds their provenance and
   authorization. Deployment targets serve copies — the release record says
   what should be live.
2. **No database transaction spans an external call** — model, render worker,
   deployment adapter, notification provider. Intent is committed to a job or
   outbox row, executed, evidenced, reconciled.
3. **The model proposes; the platform enforces.** The tool surface is
   allowlisted; every run is bounded (iterations, wall time, tokens, tool
   output, total budget); each revision requires a reason and evidence;
   imported pages and assets are untrusted content, never instructions.
4. **Approval binds to the exact candidate.** Promotion of anything but the
   approved hash is refused. Rollback is a new release; versions never
   mutate.
5. **A lead and its notification outbox commit together** before success is
   shown. The inbox is the truth; delivery is finite retries; a down provider
   loses no acknowledged lead and blocks no new submission.
6. **Only approved facts reach the page.** Testimonial and metric slots
   reference approved fact records — the generator cannot invent them. Asset
   originals and their licenses are kept.
7. **Honest evidence.** Traffic changes are separated from causal conversion
   effects; weak evidence yields "no change" or a named hypothesis — never an
   auto-published optimization. `refused` and `uncertain` are first-class
   states.
8. **Bounded everything.** Agent loops, lead intake, queues, retries, tool
   output and storage reserves all have limits and deadlines; admission is
   refused, never silently queued.

## Required failure cases

| Trigger | Required result |
|---|---|
| Model or render fails | Keep the previous approved version; expose a bounded retry; explain the failure |
| Mobile layout or form breaks | Block promotion on test failure; screenshots alone are not functional verification |
| Notification provider unavailable | Lead remains in the inbox; finite delivery retries do not block new submissions |
| Weekly evidence is weak | Recommend no change or an explicit hypothesis; do not auto-publish unsupported optimization |

## Ship gate

Ship when clients can publish a site they accept, receive real authorized
inquiries, and understand what the system changed and why — the guide's
gate — met with the workspace evidence standard: the four failure cases
drilled, the load results committed with machine + commit, and the
three-client pilot completed on the first-release scope.

## Evidence plan

Published pages are benchmarked separately from generation. 50 synthetic
briefs measure completion rate, median human corrections, generation latency
and cost. The lead API runs an offered-load scenario (the guide proposes
50 submissions/s) **with the notification provider down**: no acknowledged
loss, no duplicate logical lead. Page-load and interaction budgets are
recorded per template, not inferred from Rust. Keyboard navigation and
representative mobile devices are tested. Full load runs follow the workspace
standard — million-scale where a unit exists (lead submissions, published-
page serves); results land in `perf/results/` with machine + commit. Numbers
in the guide are proposed targets, not measured performance; laptop numbers
are development observations, never production capacity.
