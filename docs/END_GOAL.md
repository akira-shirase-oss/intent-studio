# Intent Studio — end goal

**A client brief becomes a designed, tested, published website — and
evidence, not optimism, decides what changes next.**

An operator serves a service business that wants a concrete outcome, such as
qualified quote requests. The system records the client's intent, audience,
offer, approved claims, brand and reference permissions; generates a typed
page tree from tested components; builds it into a static bundle; renders
and checks that bundle at mobile and desktop sizes; and presents a strong
first preview — the client sees the candidate, not the agent's internal
iteration. Approval binds to the hash of the exact bundle that was checked;
a scoped deployment adapter promotes those bytes, with a rollback path.
Leads land in an inbox that survives a dead notification provider.
Improvement is a review, not an autopilot: weak weekly evidence correctly
recommends no change.

Contract: the engineering guide `06_Intent_Studio.docx` (outside this
repository). Where this file and the guide disagree, the guide wins unless a
`D-NNN` in `DECISIONS.md` says otherwise. Per the guide, the existing
prototype is not assumed production-ready — the Rust backend is the new
proposal, not a port of working code. Every number below is a proposed lab
workload or acceptance target, not a measurement.

## In scope (the first useful release)

- One local-service category, one quote-request site pattern, desktop and
  mobile review and a lead inbox — piloted with three reachable clients. The
  category is chosen from the clients that exist (D-026), not the other way
  round.
- Brief and facts intake: intent, audience, offer, approved claims, brand,
  reference permissions. Every factual claim is a fact record with source,
  approver and a review-by date (D-013). Asset originals are kept with their
  license and provenance.
- Reference import: client-named URLs are fetched only through the egress
  guard (D-012) and are untrusted content, never instructions.
- Generation: provider-neutral model calls → typed page tree → deterministic
  static build → Playwright functional, accessibility and visual checks at
  three viewports (D-022) → bounded repair loop, compared with a
  deterministic baseline.
- A candidate is the built, content-addressed bundle plus its check record;
  approval binds the bundle hash (D-010). Immutable versions, reviewable
  diffs, release IDs.
- Preview first: previews are access-controlled and `noindex`; leads from a
  preview are test leads (D-027). Promotion under a scoped deploy token
  uploads the approved bytes (D-017); rollback is a new release.
- Lead capture API and inbox: the lead and its notification outbox commit in
  one transaction before success shows; one logical lead per submission ID;
  finite retries on a fixed schedule; abuse limits; tenant isolation;
  retention and deletion (D-019, D-020).
- Improvement review: authorized, cookieless observations (D-024) → a
  pre-registered decision rule (D-023) → one measured proposal, a named
  hypothesis or an explicit no-change, queued for review and never
  auto-published.
- Keeping approved facts current (guide §Release and upkeep): a stale fact
  blocks promotion and is flagged on live sites (D-013).
- The SvelteKit workspace: brief, candidate review, approval, lead inbox,
  evidence. No native shell.

## Out of scope (each needs an explicit contract)

- Additional site patterns and categories, in-app content editing,
  source-aware lead analytics (guide §Full direction).
- Health monitoring of published sites. The guide's full direction calls it
  frequent; it is not part of the first release and nothing here depends on
  it (D-028).
- The optional Colophon content adapter.
- Automatic conversion gains and daily redesigns — the guide excludes both;
  they are never promised.
- Causal experiment claims on low-volume sites — declared unsupported, not
  faked.
- AI lead scoring: qualification is rule-based in v1 (D-019).

## Non-negotiable invariants

1. **PostgreSQL is the only authority** for briefs, facts, design specs,
   candidates, check records, approvals, releases, leads, the outbox,
   budgets and evidence. The content-addressed store holds asset and bundle
   bytes; Postgres holds their provenance and authorization. Deployment
   targets serve copies — the release row says what should be live.
2. **No database transaction spans an external call** — model, render
   worker, deployment adapter, notification provider, reference fetch.
   Intent is committed to a job or outbox row, executed, evidenced,
   reconciled; `uncertain` is never relabeled success.
3. **The model proposes; the platform enforces.** Allowlisted tools; every
   run bounded (iterations, wall time, tokens, tool output, total budget,
   D-021); each revision carries a reason and evidence; imported pages and
   assets are untrusted content, never instructions; generated links point
   only at allowlisted targets (D-014).
4. **Approval binds the exact built bundle.** The approved hash covers the
   bytes that were rendered and checked; promotion deploys those stored
   bytes with no rebuild. Promotion is refused for any other hash, for a
   bundle without a passing check record for that exact hash, and for a
   bundle that uses a stale fact. Rollback is a new release; versions never
   mutate.
5. **A lead and its notification outbox commit together** before success
   is shown. The inbox is the truth; one logical lead per
   `(site, submission_id)`; delivery is finite retries; a down provider
   loses no acknowledged lead and blocks no new submission.
6. **Only approved, current facts reach the page.** Every factual-claim slot
   — testimonials, metrics, prices, guarantees, licences, certifications
   and insurance, service areas, opening hours, contact details, legal
   promises — references an approved fact record. Free-text copy is linted
   for unbound claims; a fact past its review-by date blocks promotion
   (D-013). Asset originals and their licenses are kept.
7. **Honest evidence.** Traffic changes are separated from causal
   conversion effects; weak evidence yields "no change" or a named
   hypothesis under a pre-registered rule (D-023) — never an auto-published
   optimization. `refused` and `uncertain` are first-class states.
8. **Bounded everything.** Agent loops, lead intake, queues, retries, tool
   output, fetch sizes and storage reserves all have limits and deadlines;
   admission is refused, never silently queued.
9. **Untrusted input never reaches private networks, and secrets never
   reach logs.** Every fetch of a client-named URL passes the egress guard
   (D-012); the render worker has no network beyond the bundle it checks;
   deploy tokens never enter the worker (D-017); secrets, approval tokens
   and lead PII never appear in logs (D-029).

## Required failure cases (contract, not options)

The guide's four cases. Each row names the test or drill and the condition
that passes it; drills run in CI from the milestone shown.

| Trigger | Required result | Proven by | Pass condition | From |
|---|---|---|---|---|
| Model or render fails | Keep the previous approved version; expose a bounded retry; explain the failure | Drill `generation-fails` over recorded fixtures: provider timeout, provider 5xx, invalid structured output, budget exhausted, render worker killed mid-job, check suite failing | The live release row and the deployed bundle hash are unchanged; the job ends in a named failure code (D-021) shown in the workspace with its explanation; at most 2 automatic retries, only for transient codes; a manual retry starts a new bounded job | I2 |
| Mobile layout or form breaks | Block promotion on test failure; screenshots alone are not functional verification | The promotion check suite (D-022) against `fixtures/broken-layout/` (overflow at 360 px, a submit that never reaches the server, an unlabeled required field, a tap target under 24×24 px, a keyboard trap), plus API tests on `approve` and `promote` | Every fixture yields a failing check record; `approve` and `promote` both answer `409 checks_failed` for that bundle hash; a bundle with no check record for its exact hash cannot be approved or promoted; zero promotions of a failing fixture | suite I2, gate I3 |
| Notification provider unavailable | Lead remains in the inbox; finite delivery retries do not block new submissions | Drill `notify-down`: the provider fixture answers 503 or hangs for the whole run while k6 submits leads, 5% of them deliberate resubmissions | Every lead acknowledged with 201 exists exactly once in the inbox (post-run reconciliation of k6's acknowledged IDs against the table); zero duplicate logical leads; intake p99 within the budget set from the I0 baseline (D-054); every outbox row follows the D-020 schedule and ends `delivered` after recovery or `dead` (visible, with manual resend) after 8 attempts | smoke I0, full run I3 |
| Weekly evidence is weak | Recommend no change or an explicit hypothesis; do not auto-publish unsupported optimization | Review-rule tests (D-023) over `fixtures/evidence/`: tiny counts, a traffic-only rise, a noisy series and one large synthetic true effect; an architecture check that `intent-review` cannot depend on `intent-publish` | Every weak fixture yields `insufficient_evidence` with `no_change` or a named hypothesis; only the large fixture may report an observed difference, and even then the output is a queued proposal; no code path leads from review to promotion | I4 |

### Security cases (required by the invariants)

| Case | Proven by | Pass condition | From |
|---|---|---|---|
| A reference URL that resolves to loopback, RFC 1918, link-local or cloud-metadata addresses; DNS rebinding; a redirect to a private target | Egress-guard tests with a fixture resolver (D-012) | Each refused with a named code before any connection is opened; the guard has no mode that disables it | I1 |
| A hostile reference page injects links or instructions | `fixtures/hostile-reference/` through the full generation path | No link outside the allowlist validates (D-014); the run's tool calls match the benign fixture's | I2 |
| Invented claims in free text ("licensed & insured", "$99", "since 1985") | Claim-lint fixtures (D-013) | The check record fails with `unbound_claim`; promotion is refused | I2 |
| A fact past its review-by date | Clock-fixture tests (D-013) | Promotion refused with `fact_stale`; a live site using it is flagged in the inbox | validator I1, gate I3 |
| An approval link replayed, used after its TTL, or prefetched by a mail scanner | Approval-link tests (D-015) | The second use and the expired use are refused; a GET never approves; the token never appears in logs | I3 |
| A lead submitted from a preview | Preview-lead test (D-027) | Stored with the test flag; no client notification is sent | I3 |
| An SVG carrying script, a malformed image or font | Asset fixtures (D-011) | SVG rasterized or sanitized in the worker, no script in any bundle; a decoder crash fails that job only | I1 |
| Secrets and PII in logs | Log-capture test over every drill (D-029) | Zero matches for token shapes, approval and deploy tokens, and the fixture leads' emails and phone numbers | I0 onward |

## Ship gate

The guide: **ship when clients can publish a site they accept, receive real
authorized inquiries and understand what the system changed and why.** In
testable form, all of these hold at I6:

1. **Accept.** Each of the three pilot clients signs off a specific release
   through the approval mechanism; the acceptance row names the release ID
   and the bundle hash that is live.
2. **Real inquiries.** Each pilot site receives at least one real, non-test
   inquiry within 8 weeks of its first promotion, and the client confirms
   it is genuine.
3. **Understand.** After the latest change, each client names what changed
   and why, using only the workspace change log, in a short recorded check;
   the answer matches the release record (the change and its recorded
   reason).
4. **Engineering evidence on the release commit.** The four failure cases
   and the security cases pass; the full 1,000,000-submission run with the
   provider down, the 50-brief battery and the per-template page budgets are
   committed under `perf/results/` with machine and commit.

A miss on 1–3 is recorded as a pilot outcome, not rerun until it looks good.
The gate is blocked until the pilot clients exist (README, B-1).

## Evidence plan

- **Lead intake — the 1,000,000-unit run.** The unit is one lead
  submission to `intent-server` on the test machine, with the notification
  provider fixture down for the whole run and 5% deliberate resubmissions.
  The guide's 50 submissions/s is run as an open-model realism check, plus a
  5× burst (250/s); the full run uses the highest rate that holds the I0
  budgets and records that rate. Pass conditions are failure case 3's.
- **No third party is load-tested.** Published pages are served by the
  deploy target, so they are never benchmarked there; the model, email and
  deploy providers receive only functional traffic (D-025).
- **Published pages.** Built bundles are served by a local static server;
  Lighthouse mobile and desktop profiles per template, three runs, median
  kept; budgets for LCP, CLS, TBT and transfer size are set from the first
  measured baseline and fail the check on a miss.
- **Keyboard and mobile.** Keyboard-only completion of the form and the
  three-viewport checks are part of the promotion suite (D-022), not a
  final extra; each template is also checked once on a real iOS Safari and
  a real Android Chrome device, recorded with device and OS version.
- **Generation.** 50 synthetic briefs run against recorded model responses,
  so the run is reproducible: completion rate, median human corrections,
  latency, budget use. One real-provider pass reports latency and cost
  separately (needs B-2). The repair loop is compared with the
  deterministic baseline (component defaults filled from fact records, no
  model), not rerun until it looks good.
- **Observation beacon.** Its own scenario at I4: 1,000,000 beacon events
  against `intent-server`, bounded aggregation, no per-visitor rows.
- Every result records the machine, the commit and the machine load
  (`uptime`); laptop numbers are development observations, never
  production capacity.

## Release and upkeep

From the guide: generation prompts, templates and component sets are
versioned data, and every candidate records the versions it was built with;
design regressions are tracked across template versions (visual snapshots
plus the check suite); deploy-adapter compatibility is tested per release;
imported-asset handling keeps its fixture tests; approved client facts are
kept current (D-013); queued proposals are reviewed rather than forcing
weekly edits.
