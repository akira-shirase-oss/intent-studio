# Roadmap

Step order follows the guide's implementation table — brief and components,
reference-to-preview, reliable launch, improvement review — preceded by I0,
the spine every project in this workspace starts with (CI, a load scenario,
a failure drill), and followed by the evidence battery and the pilot. The
mapping: guide step 1 → I1 · step 2 → I2 · step 3 → I3 · step 4 → I4 · the
performance and load plan → I0, I3 and I5 · the ship gate → I6 (D-026).

**Status rule.** A status cell may say *Done* only when it names the tests
and the evidence path that prove the gate. Anything else is *Planned*,
*In progress* or *Blocked (B-n)*. Load evidence measured on a busy machine
is a smoke observation, never capacity, and the full run stays listed as
owed.

Every milestone ships: regression tests; its failure drills as automated
tests in CI; a load scenario with thresholds under `perf/` for anything new
that serves traffic; the log-capture test over its drills.

| Step | Builds | Proves (the gate) | Status |
|---|---|---|---|
| **I0 — the spine** | Cargo workspace and SvelteKit shell; `scripts/check.sh` and a CI job running it (fmt, clippy `-D warnings`, tests, `svelte-check`, the architecture checks below); Postgres schema for tenants, sites, leads and the outbox; `POST /v1/sites/{site}/leads` with body cap, honeypot, per-site and per-IP limits, `(site, submission_id)` uniqueness and one lead+outbox transaction; the leased notifier with the D-020 schedule; the provider fixture; the operator inbox (read-only list); the first k6 scenario | CI green on the first commit; **drill `notify-down`** (failure case 3, smoke size: 100,000 submissions) passes its pass condition; **drill `db-down`**: Postgres stopped mid-run → intake answers 503 with `Retry-After`, never 201, and after restart every acknowledged lead is present; the 50/s realism check and the 5× burst run with budgets recorded as the baseline; the log-capture test passes | **In progress** — built; every drill passes in CI (green from `82073a8`); the quiet-machine baselines are recorded (realism p99 14.7 ms, burst p99 89.9 ms) and the 100,000- and 1,000,000-submission runs with the provider down passed at `e61d0c2` (p99 5.4 ms and 12.1 ms against the 100 ms threshold, 0 dropped, 0 duplicates, 0 violations; `perf/results/README.md`); owed: the decision entry that sets the intake budget from those baselines (see *I0 status*) |
| **I1 — brief, facts and components** (guide step 1) | Brief, fact and reference schema; fact records with source, approver and review-by date; the typed page-tree spec and validator; claim lint for free text; the content-addressed store; `intent-egress`; the asset worker (SVG rasterize/sanitize, image and font decoding); reference notes. Second half: the hand-built component set for the one chosen category | One validated brief round-trips through the API; every invalid brief or spec is a named refusal; a claim slot referencing an unapproved or stale fact cannot validate; the egress-guard and asset security cases pass; second half: components render a spec deterministically (same input → same bundle bytes) at all three viewports | Planned; component set Blocked (B-1) |
| **I2 — reference → preview** (guide step 2) | The provider-neutral model port with recorded-response fixtures; structured output → page tree; the bounded loop with D-021 limits; revision records; the isolated render/check worker (D-011); the promotion check suite (D-022); the bounded repair loop; the deterministic baseline | **Drill `generation-fails`** (failure case 1) passes; the check suite fails every `broken-layout` fixture; the hostile-reference and claim-lint cases pass; the repair loop is compared with the deterministic baseline over the synthetic briefs and the result is kept, whichever wins; Chromium memory measured and the worker concurrency set from it; real-provider pass when B-2 is resolved | Planned |
| **I3 — reliable launch** (guide step 3) | Candidates keyed by bundle hash; access-controlled `noindex` previews with test leads; approval links (D-015); reviewable diffs and release IDs; the `local-static` and Cloudflare Pages adapters (D-017) with read-back; promotion's four checks; rollback; retention and deletion (D-019); CSV export with formula escaping | **Failure case 2 gate:** `approve` and `promote` refuse every failing or unchecked bundle; the approval-link, preview-lead and stale-fact cases pass; **rollback drill** on a real deploy to an operator-owned test project: promote A, promote B, roll back to A, read-back shows A's manifest; **full 1,000,000-submission `notify-down` run** committed under `perf/results/` | Planned |
| **I4 — improvement review** (guide step 4) | The cookieless beacon (D-024); daily aggregation; the versioned decision rule (D-023); the proposal queue — one measured change with its hypothesis, a named hypothesis, or `no_change` | **Failure case 4** passes over the evidence fixtures; the architecture check keeps `intent-review` away from `intent-publish`; the beacon's 1,000,000-event run is committed with bounded aggregation and no per-visitor rows | Planned |
| **I5 — evidence battery** | The 50-brief battery (completion rate, median human corrections, latency, budget use); Lighthouse budgets per template; real-device checks (one iOS Safari, one Android Chrome); every drill re-run on one commit | All four failure cases and all security cases pass on the same commit; per-template budgets hold; results committed with machine + commit + `uptime` | Planned |
| **I6 — pilot and ship gate** | Three client sites on the first-release scope: briefs, approvals, promotions, real leads, at least one weekly review each | The ship gate in `END_GOAL.md`: signed-off releases, a real inquiry per site within 8 weeks, the recorded "what changed and why" check, the engineering evidence on the release commit | Blocked (B-1, B-3, B-4) |

### I0 architecture checks

Run by `scripts/check.sh` in CI from the first commit:

- `intent-domain` has no I/O dependency (no tokio, sqlx, reqwest, hyper or
  std::fs use);
- only `intent-agent` may depend on a model-provider client;
- only `intent-egress` may depend on an HTTP client for client-named URLs;
- `intent-review` may not depend on `intent-publish`;
- the workspace app never imports a database client.

### I0 status

*In progress* (2026-11-17). Not *Done*: the gate's load half needs a
baseline from a quiet machine, and none was available — every run below was
made at load average 16–78 with other projects building.

Proven, each by a named test run in CI on every push (`scripts/check.sh`;
green on `82073a8`, `49db27b`, `50bc7d0`, `a5e0d6f`, `5df19c5`):

- CI from the first commit: toolchain pin, fmt, clippy `-D warnings`,
  tests, the five architecture rules and a self-test that plants a
  violation of each (`scripts/test-architecture-check.sh`), cargo-deny,
  svelte-check, the workspace build.
- Lead intake, the outbox and the notifier as specified in
  `docs/modules/01-lead-intake.md`, with every claim mapped to its test.
- **Drill `notify-down`** at CI size (2,750 submissions, 5%
  resubmissions, provider refusing then hanging):
  `drill_notify_down::notify_down_loses_and_duplicates_nothing_and_never_blocks_intake`,
  plus the never-recovers and lost-answer cases.
- **Drill `db-down`**: `drill_db_down::db_down_refuses_honestly_and_recovers_without_loss`
  — PostgreSQL stopped mid-run, only `503` with `Retry-After` while down
  (p99 4.7 ms), readiness red then green, no loss after restart.
- `kill -9` mid-request: `drill_kill::kill_9_mid_request_loses_no_acknowledged_lead`.
- The log-capture test over every drill (`support::assert_no_leaks`, which
  first proves it catches a planted leak).
- The operator inbox in Chromium at 1280 and 360 px
  (`scripts/workspace-e2e.sh`).

Load (`perf/results/`, index in its README; development observations on a
busy laptop, never capacity):

- 50/s realism check: all 3,001 reconciled; p99 402 ms — a miss on a busy
  machine (load average 27–57).
- 5× burst, 250/s: all 15,001 reconciled, outbox = provider; p99 15.3 ms —
  held. (An earlier burst run is kept and marked invalid: a stray process
  of mine shared its database; D-037.)
- `notify-down`, 100,000 submissions at 500/s: correctness held — every
  acknowledged lead stored once, 0 violations, all 94,269 notifications
  delivered after recovery with 0 duplicates; p99 109 ms and 771 dropped
  iterations — misses at load average 16–78.

Fix round (2026-11-17), after an independent review; each fix with a test
that failed first, `scripts/check.sh` green and CI confirmed: connection
bounds and no body read before the cheap refusals (D-038); the site budget
spent only by storable leads for real sites (D-039); every
`X-Forwarded-For` line read; outbox records fenced per claim (D-040); the
notifier on its own pool for every query, sized for its concurrency
(D-048), and readiness on its own connection (D-047); operator sign-in
hardened (D-041); bidi and invisible characters refused (D-042); CSP and
framing protection (D-043); the consent notice version stored (D-044); the
recorder refused outside development (D-045); provider answers retried by
class (D-046); `/metrics` restricted (D-049); an argon2id floor (D-050);
scripts that run the binaries cargo built; a leak check that proves every
needle; grouped imports in the architecture check. The load results above
predate the fix round and were not rerun (the machine stayed at load
average 13–45).

Owed before *Done*: realism and burst on a quiet machine (`uptime` below
about 6) and the intake budget set from them in a decision entry; the
100,000-submission `notify-down` smoke against that budget. Owed by I3: the
1,000,000-submission run (`scripts/load.sh full`).

## Later extensions (explicit contracts required)

Additional site patterns and categories, in-app content editing, source-aware
lead analytics, health monitoring of published sites, the optional Colophon
content adapter, client accounts in place of approval links. Each gets its
own contract before work starts. The guide excludes automatic conversion
gains and daily redesigns outright.
