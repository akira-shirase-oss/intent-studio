# Architecture

## Sources of truth

1. **PostgreSQL is the only authority** — briefs, approved facts, design
   specs, page trees, candidates, approvals, releases, leads, the
   notification outbox, budgets and evidence records.
2. **Object storage holds asset bytes** — originals, immutable and
   content-addressed. Postgres holds the provenance, license and permission
   records that say which bytes may appear on a site.
3. **Deployment targets hold served copies** — the adapter (scoped
   GitHub/Vercel or alternative) pushes each release outward; the release row
   in Postgres is authority for what should be live. A reachable preview
   proves nothing about promotion.
4. **Nothing in the agent loop is authoritative** — model output becomes real
   only when validated into a stored candidate. Run state is rebuilt from
   Postgres on restart; a dead worker leaves a resumable job, not a mystery.

## Components

| Piece | Role |
|---|---|
| `intent-domain` | Pure types and transitions: brief, fact set, design spec, page tree, candidate, approval, release, lead, proposal; the invalid-state list; all limits. No I/O — enforced. |
| `intent-db` | Migrations and access: tenant-scoped queries, outbox, the release ledger, the lead inbox, evidence records. |
| `intent-agent` | The bounded agent loop: provider-neutral model port, allowlisted tools, budget metering, structured-output validation, revision records (reason + evidence). |
| `intent-render` | Isolated render/build workers: page tree → static site; Playwright functional + visual checks at two viewports; defect reports flow back to the loop. |
| `intent-publish` | Deployment adapters: preview, promote, rollback; release diffs; re-verifies the approved hash at promotion. |
| `intent-server` | Axum API: workspace endpoints, lead intake, provider webhooks; serves the built workspace app on the same origin. |
| `apps/workspace` | SvelteKit + TypeScript workspace: brief entry, candidate review, approval, lead inbox, evidence. Client-side; holds no authority. |
| `packages/site-components` | The tested component set for the first site pattern, with per-template page budgets. |

## Paths

**Brief → spec:** `POST /briefs` → validate intent / audience / offer /
approved claims / brand / reference permissions → a design-spec row. Facts
are records, not prompt flavor: claim and metric slots reference fact ids,
and an unapproved reference is a named refusal.

**Generate → candidate:** a bounded job claims the spec → model calls through
the provider-neutral port (structured output validated into the page tree —
invalid output is a named refusal, not an endless retry) → the render worker
builds a static site → Playwright functional + visual checks at desktop and
mobile → defects feed the bounded repair loop, each revision carrying its
reason and evidence → a candidate row with its check results. The previous
approved version is untouched the whole time; a failed run keeps it and says
why.

**Approve → publish:** approval stores the candidate's content hash → a
release row + reviewable diff → the restricted worker builds → the adapter
deploys a preview → promotion requires the scoped permission and re-verifies
the hash → live. Rollback is a new release pointing at a prior version;
nothing mutates in place.

**Lead:** `POST {site}/leads` → validate + abuse limits → **one transaction
commits the lead and its notification outbox** → success shown → the notifier
retries finitely. A down provider leaves the lead in the inbox and never
blocks new submissions.

**Review:** authorized observations collect per release → the review job →
a queued proposal (one measured change + its hypothesis) or an explicit
no-change → human review. Nothing auto-publishes; low-volume sites get an
honest "insufficient evidence", not a fabricated experiment.

## Consistency story

Every state change is a Postgres transaction; every external effect goes
through a job/outbox row with a deadline and finite retries, so a crash
mid-generate or mid-publish resumes or reconciles instead of half-applying.
Approvals compare content hashes — a candidate that drifted cannot promote
under a stale approval. Uncertain external outcomes (deploy accepted but
unconfirmed, notification result unknown) stay `uncertain` until reconciled;
they are never relabeled success. Tenant isolation is an `intent-db`
concern: every query carries a tenant scope, and a path that crosses tenants
is a defect with a regression test, not a behavior.

## Layout

```
crates/                  intent-domain, intent-db, intent-agent,
                         intent-render, intent-publish, intent-server
apps/workspace/          SvelteKit workspace (review, approval, inbox, evidence)
packages/site-components/  tested components + per-template budgets
docs/                    END_GOAL, ARCHITECTURE, ROADMAP, DECISIONS, modules/
fixtures/                synthetic briefs, recorded model responses,
                         broken-layout and provider-down harnesses
perf/                    k6 scenarios (lead intake, published-page serving),
                         the generation battery, results/
```

## Failure cases

See `END_GOAL.md` — the four required cases are contract, not options:
model/render failure keeps the approved version; a broken mobile layout or
form blocks promotion; a down notification provider loses no lead; weak
evidence recommends no change.
