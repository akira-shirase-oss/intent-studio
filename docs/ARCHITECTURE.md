# Architecture

Planned architecture — no code exists yet. Crate and package names are
fixed at I0.

## Sources of truth

1. **PostgreSQL is the only authority** — tenants, briefs, approved facts,
   design specs, page trees, candidates, check records, approvals,
   releases, leads, the notification outbox, budgets and evidence records.
2. **The content-addressed store holds bytes** — asset originals and built
   bundles, immutable, addressed by SHA-256, behind a storage port. The
   pilot backend is the local filesystem, backed up with the database
   (D-016). Postgres holds the provenance, license and permission records
   that say which bytes may appear on a site.
3. **Deployment targets hold served copies** — the adapter uploads a stored
   bundle; the release row is the authority for what should be live. After
   each deploy the adapter reads the deployed manifest back and records it;
   a mismatch is `uncertain` until reconciled. A reachable preview proves
   nothing about promotion.
4. **Nothing in the agent loop is authoritative** — model output becomes
   real only when validated into a stored page tree. Run state is rebuilt
   from Postgres on restart; a dead worker leaves a resumable job.

## Components

| Piece | Role |
|---|---|
| `intent-domain` | Pure types and transitions: tenant, brief, fact, design spec, page tree, candidate, check record, approval, release, lead, observation, proposal; the invalid-state list; every limit. No I/O — CI enforces it. |
| `intent-db` | Migrations and access: tenant-scoped queries, the outbox, the release ledger, the lead inbox, evidence records. |
| `intent-egress` | The one way to fetch a client-named URL: address policy, DNS pinning, redirect revalidation, size and time caps (D-012). |
| `intent-agent` | The bounded agent loop: provider-neutral model port (the only place a provider client may be called), allowlisted tools, budget metering, structured-output validation, revision records. |
| `intent-render` | Job driver for the isolated worker (D-011): asset processing, the deterministic static build, the check suite; results flow back as a check record. |
| `intent-publish` | Deployment adapters (`local-static` for CI and drills; Cloudflare Pages direct upload, D-017): promote, rollback, read-back reconciliation. Holds the only decryption path for deploy tokens. |
| `intent-review` | Observation aggregation and the versioned decision rule (D-023). May not depend on `intent-publish` — CI enforces it. |
| `intent-server` | Axum API: workspace endpoints, approval links, lead intake, the observation beacon, preview serving; serves the built workspace app on the same origin. |
| `apps/workspace` | SvelteKit + TypeScript workspace: brief entry, facts, candidate review, approval, lead inbox, evidence. Client-side; holds no authority. |
| `packages/site-components` | The tested Svelte component set for the first site pattern, built to static output; per-template page budgets. |
| `packages/checks` | The Playwright + axe-core promotion suite (TypeScript, run inside the worker) and the Lighthouse budget runner. |

## Paths

**Brief → spec:** `POST /v1/briefs` → validate intent, audience, offer,
approved claims, brand, reference permissions → a design-spec row. Facts are
records with a source, an approver and a review-by date; claim slots
reference fact IDs; an unapproved or stale fact is a named refusal.

**Reference import:** a client-named URL → `intent-egress` (policy check,
pinned DNS answer, at most 3 redirects each revalidated, 10 s deadline,
5 MiB HTML / 20 MiB asset caps) → the raw bytes go to the content-addressed
store with provenance → parsing and image decoding happen in the worker,
never in the API process. The extracted text is data for the model, marked
untrusted.

**Generate → candidate:** a bounded job claims the spec → model calls
through the port (structured output validated into a page tree — invalid
output is a named refusal) → the worker builds the static bundle from the
page tree and the pinned component-set version (no generated code runs) →
the bundle is stored by hash → the check suite runs against **those bytes**
at 360, 390 and 1280 px → defects feed the bounded repair loop, each
revision with its reason and evidence → a candidate row = page-tree hash +
component-set version + bundle hash + check record. The previous approved
version is untouched; a failed run keeps it and says why.

**Preview → approve:** the server serves the stored bundle as a preview
(operator session or approval link only, `X-Robots-Tag: noindex`, forms
post as test leads) → the client opens a single-use approval link bound to
the bundle hash → a confirm `POST` (a `GET` never approves) records the
approval with the approver and the hash.

**Promote → live:** promotion re-checks four things in one transaction —
the approved hash, a passing check record for that hash, no stale fact, the
scoped permission — then writes the release row and an outbox job → the
adapter uploads the stored bytes (no rebuild) → reads back the deployed
manifest → `live` or `uncertain`. Rollback is a new release pointing at a
prior stored bundle; nothing mutates in place.

**Lead:** the static form posts to `POST /v1/sites/{site}/leads` (CORS
limited to the site's release origins; a no-script form gets a 303 to the
thank-you page) → body cap, honeypot, per-site and per-IP limits → **one
transaction commits the lead and its notification outbox row**, unique on
`(site, submission_id)` → success → the notifier claims outbox rows under a
lease and retries on the D-020 schedule. A down provider leaves the lead in
the inbox and never blocks new submissions.

**Review:** the cookieless beacon (D-024) aggregates page loads, form views
and leads per site, release and day → the weekly review job applies the
versioned rule (D-023) → a queued proposal, a named hypothesis or
`no_change` → human review. Nothing auto-publishes.

## Egress and isolation

- What needs isolation is untrusted input processing, not generated code:
  image, SVG and font decoding, reference HTML parsing, and Chromium loading
  the built page (D-011).
- Each worker job runs in a rootless container: read-only root filesystem,
  a scratch directory, no credentials, CPU, memory and wall-time limits.
  The render and check job has **no network** beyond the local server that
  serves the bundle under test; bundles are self-contained (no third-party
  subresources), so nothing else is needed. Import jobs reach the internet
  only through `intent-egress`.
- Deploy tokens are encrypted at rest and decrypted only inside
  `intent-publish`, never passed to a worker (D-017).
- On a small single host, render jobs run one at a time; Chromium's memory
  is measured at I2 and sets the concurrency, not a guess.

## Actors and tenancy (D-015)

- A tenant is one client business. Every row carries a tenant ID; every
  query in `intent-db` takes a tenant scope; a cross-tenant path is a
  defect with a regression test.
- The operator (the maintainer) signs in with a password session and acts
  across tenants; every cross-tenant action writes an audit row.
- Client approvers have no accounts in the pilot. They act through
  single-use approval links (48 h TTL) bound to the approver contact and the
  bundle hash, stored as SHA-256 digests and never logged.

## Consistency story

Every state change is a Postgres transaction; every external effect goes
through a job or outbox row with a deadline and finite retries, so a crash
mid-generate or mid-publish resumes or reconciles instead of half-applying.
Approvals and promotions compare bundle hashes — a candidate that drifted
cannot promote under a stale approval, and because promotion uploads stored
bytes, nothing can drift between check and deploy. Uncertain external
outcomes (deploy accepted but unconfirmed, notification result unknown)
stay `uncertain` until reconciled; they are never relabeled success.

## Layout

```
crates/                    intent-domain, intent-db, intent-egress,
                           intent-agent, intent-render, intent-publish,
                           intent-review, intent-server
apps/workspace/            SvelteKit workspace
packages/site-components/  tested components + per-template budgets
packages/checks/           Playwright + axe-core suite, Lighthouse runner
docs/                      END_GOAL, ARCHITECTURE, ROADMAP, DECISIONS;
                           modules/ is added when the first module lands
fixtures/                  synthetic briefs, recorded model responses,
                           broken-layout, hostile-reference, evidence
                           series, provider-down harness, asset fixtures
perf/                      k6 scenarios (lead intake, beacon), the
                           generation battery, results/
scripts/                   check.sh (what CI runs), drills
```

## Failure cases

See `END_GOAL.md` — the four required cases and the security cases are
contract, each with its test and pass condition.
