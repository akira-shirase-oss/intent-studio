# Plan — I2: reference → preview

Guide step 2: a brief becomes a candidate — a typed page tree built by a
bounded model loop, rendered and checked in an isolated worker, recorded as
immutable bytes plus a check record. Scope and gate per `ROADMAP.md`;
invariants per `END_GOAL.md`; the decisions this plan leans on are D-010
(approval binds the built hash), D-011 (worker isolation), D-012 (egress —
built), D-013/D-014 (claims and links — the validator is built), D-021
(bounded loop + failure taxonomy), D-022 (what "breaks" means).

## What exists and what I2 adds

Built already: `intent-domain::page_tree` — the typed `quote-request@0`
spec, parsed and validated against a brief's facts (claim lint, link
allowlist, bounds, staleness) exposed at
`POST /v1/operator/tenants/{tenant}/briefs/{brief}/spec-check`. The confined
asset worker (`intent-render` driver + `intent-worker`: sanitized SVG,
re-encoded images, checked fonts — the NEW-4 header protocol is proven).
The egress-guarded reference snapshot table. Briefs, facts, revisions.

I2 adds, in dependency order:

1. **`intent-agent`** — the provider-neutral model port. Recorded-response
   fixtures only at first (a `Recorded` provider whose transcripts are
   versioned data); a real provider arrives when B-2 resolves. The port is
   the only place a model client may live — the existing architecture check
   already reserves the name.
2. **Generation jobs** — `generation_jobs` (tenant, brief, status, failure
   code, attempts, budgets, prompt/template/component versions) and
   `generation_revisions` (one row per iteration: reason + evidence).
   Intent commits to the job row first; no transaction spans a model or
   worker call (invariant 2). Job states name the D-021 codes; `uncertain`
   is first-class and reconciled, never relabelled — a killed worker
   mid-check is `render_crashed`, not a guess.
3. **Structured output → page tree** — model output parsed into the
   existing validator. Out-of-schema or failing validation is
   `model_invalid_output` — a named refusal, never a retry-forever.
4. **The deterministic builder** — page tree → static bundle bytes,
   byte-stable under pinned fonts/dates/asset hashing; the same input
   produces the same SHA-256 (the determinism proof). **Depends on the
   `quote-request@0` renderers — I1's second half, still Blocked (B-1).**
5. **Candidates and check records** — `candidates` (bundle SHA-256 → the
   store, tree JSON, exact versions used) and `check_records` (bundle hash,
   suite version, per viewport × engine results, verdict). A check record
   binds its exact hash; `approve`/`promote` require a passing record for
   that hash — the D-010 seam, enforced before I3 uses it.
6. **The check worker** — D-011's other half: a bundle under test loads in
   Chromium and WebKit at 360×800, 390×844, 1280×800, no network beyond the
   loopback server that serves only that bundle, no credentials, CPU /
   memory / wall-time bounds, Chromium memory measured → concurrency set
   from the measurement. D-022's checks: no horizontal overflow; the form
   submits to a local `intent-server` and shows success only after the
   201; required fields announce errors (`aria-invalid`, `aria-describedby`,
   live region); zero critical/serious axe-core violations; tap targets
   ≥24×24; every interactive reachable by Tab with visible focus and the
   form completable by keyboard; visual snapshots stored for review, never
   sufficient alone.
7. **The bounded loop** — repair iterations ≤3, 10 min wall, 200k tokens,
   US$2, 256 KiB tool output per call (D-021); each revision carries its
   reason and evidence. Only `model_timeout`, `model_unavailable` and
   `render_crashed` auto-retry, at most twice; a manual retry opens a new
   bounded job.
8. **The deterministic baseline** — components filled from fact records,
   no model. The repair loop is compared with it over the synthetic-brief
   fixtures and the result kept, whichever wins (the gate's own words).
9. **`generation-fails` drill** — failure case 1: fixture provider
   timeout, fixture 5xx, malformed structured output, budget exhaustion, a
   worker killed mid-job, a bundle that fails checks. Each ends in its
   named code, the current candidate pointer never moves, ≤2 automatic
   retries for transient codes only.

## Failure cases and attack classes (each goes into the mutation proof)

| Class | Refusal | Proof |
|---|---|---|
| Malformed / out-of-schema model output | `model_invalid_output` | fixture transcript; a `parse-anything` seam must turn it red |
| Output/tool call over the D-021 bounds | `budget_exhausted` | a transcript exceeding each bound; a `trust-model-budget` seam (counting only what the model reports) must go red — the bound is measured, not claimed |
| Repair loop unbounded | iteration cap | a transcript that always requests one more iteration; a `no-iteration-cap` seam |
| Hostile reference → link injection | `link_not_allowed` (D-014) | `fixtures/hostile-reference/` carrying `javascript:` and off-allowlist `https` targets; the benign fixture's tool calls must match — imported bytes are content, never instructions |
| Hostile reference → instruction injection | the model never receives instructions as instructions | the transcript shows snapshot bytes passed as data; a `follow-imported-instructions` seam must make the run differ from the benign one |
| Unbound claim in free text | `unbound_claim` check record | claim-lint fixtures (D-013): "licensed & insured", "$99", "since 1985" with no fact |
| Check record for a different hash | refusal | a `check-any-hash` seam on the candidate/check join must go red |
| Worker killed mid-check | `render_crashed`, other jobs unaffected | kill -9 the check worker; the job reconciles, the pool keeps serving |
| Inline SVG carrying script class | refused or stripped | `fixtures/inline-svg/` — see risks; a `skip-svg-sanitize` seam must go red |
| Secrets reach worker or logs | none | log-capture extended over the generation drills: provider token, prompt internals, brief internals, approval tokens |
| Transaction spanning a model/worker call | architecture | the job row commits intent before any call; a code review check like the outbox's |

## Load scenario

Nothing in I2 opens a new public-write path — generation and checks are
operator-triggered. The meaningful 1M run is **isolation under load**:

the I0 lead-intake scenario re-run at 1,000,000 submissions at the highest
rate that held the D-054 budgets, **while the check-worker pool runs
saturated at its measured concurrency and generation jobs queue**. Pass:
intake p99 inside D-054; zero lost or duplicate leads; worker concurrency
never exceeded; no generation work starves intake's, sessions' or the
notifier's pools (the pool-starvation class rounds 4–6 kept finding). The
milestone's own functional load is the synthetic-brief battery — N briefs
over recorded transcripts measuring completion rate, latency and budget
use (the 50-brief battery is I5; I2 runs the comparison set); and Chromium
memory per check job, which sets the worker bound.

## Risks

1. **Inlining sanitised SVG into bundle markup.** Served as `img`/asset the
   sanitized SVG lives in image context — it cannot script. Inlined it is
   page markup, and a miss in the allowlist is script in a first-party
   origin. Before any template inlines one, all of these must hold: the
   allowlist is closed for the inline context — no `script`, no
   `foreignObject` (HTML inside SVG), no event attributes, no
   `javascript:`/`data:` URLs in `href`/`xlink:href`/style, `<use>` and
   animation elements resolving only internal fragments, `<style>` content
   through the same CSS-function scan the worker runs; the bundle builder
   inlines only worker output bytes, never the upload; the served bundle's
   CSP treats inline SVG as markup (it does not ride `img-src`); and a
   fixture proves a hostile SVG cannot ride the inline path. **Pilot
   recommendation: raster or `img`-reference for I2; inline only after the
   allowlist is proven in the bundle context.**
2. **A browser inside the worker.** Chromium + WebKit are far bigger
   decoder surfaces than the asset worker's: memory per job must be
   measured before the concurrency bound is set; WebKit on Linux CI is a
   real operational cost; and a full browser's own sandbox may fight ours —
   the no-network guarantee needs a loopback-only bundle server plus the
   same rlimit/seccomp class the asset worker proved, not a port of it.
3. **Recorded transcripts drift from reality.** Fixture transcripts are
   versioned data, but they cannot express a real provider's streaming or
   retry quirks; the real-provider pass stays owed to B-2, reported
   separately, and the port is shaped so the transcript is one provider
   implementation, not a special case.
4. **I1's renderers don't exist.** The `quote-request@0` validator exists;
   the component renderers (I1's second half) do not — I2's builder and
   the deterministic baseline both need them. B-1 gates the whole
   milestone's end.
5. **Model-reported budgets are untrusted.** Token and cost counts come
   from the provider's own response — the enforced bound counts measured
   values where possible (fixtures make them deterministic); a real
   provider's numbers are evidence, not enforcement.
6. **Determinism.** Byte-stable bundles need pinned font versions, no
   timestamps in bytes, and content-addressed assets — the drill proves it
   by hashing the same build twice.
7. **`uncertain` first-class.** The outbox already models it; generation
   needs its own reconcile — a worker killed after writing partial check
   evidence is `render_crashed` (retryable), not ambiguous.

## END_GOAL claims the branch hasn't yet proven

- **Failure case 1** (`generation-fails`): nothing exists — no job
  machine, no failure codes, no bounded retry, no "previous version
  untouched" pointer.
- **Failure case 2's suite** (D-022): the six check kinds × three
  viewports × two engines do not exist; `fixtures/broken-layout/` does not
  exist. (The `approve`/`promote` 409s are the I3 gate.)
- **Security cases owed to I2**: hostile reference; `unbound_claim` as a
  failing check record (the lint itself is proven in the validator);
  inline-SVG class.
- **"The model proposes; the platform enforces"** for generation: the
  port, the bounds, the failure taxonomy — none exists; `intent-agent` is
  a reserved name only.
- **The deterministic baseline comparison** — neither half exists yet.
- **Chromium memory measured → worker concurrency set** — owed.
- **Revision records with reason + evidence** — new.
- **Immutable candidate + check-record-per-hash** — new; the D-010
  enforcement seam is unproven until `approve`/`promote` exist (I3), but
  the join constraint itself is testable at I2.
- Already proven elsewhere and I2 must not regress: egress refusal codes
  (I1), asset-worker's confinement and protocol bounds (I1/NEW-4),
  `uncertain` as a state (outbox), pool isolation (I0), intake's 1M run
  (`e61d0c2`), the arch rule `intent-review` ↛ `intent-publish` (enforced;
  I2 adds `intent-agent` as the only model-client home).
