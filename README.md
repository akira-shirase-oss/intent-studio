# Intent Studio

> **Status — I0 (the lead-intake spine) built and drilled in CI; its
> quiet-machine load baseline and the full 1,000,000-submission run are
> owed; ship gate blocked on pilot clients.** See `docs/ROADMAP.md` (I0
> status) for exactly what is proven and what is not. The hand-built
> component set needs the site category, and the ship gate needs three real
> pilot clients; neither exists yet (see *Blocked on*).

A service-business brief in; a designed, tested, published website out —
then evidence, not optimism, decides what changes. The model proposes; the
platform enforces: a typed page tree of tested components, only approved and
current facts on the page, approval bound to the exact built bundle that was
checked, preview-first publishing under a scoped token with rollback, and a
lead inbox that survives a dead notification provider. First release: one
local-service category, one quote-request site pattern, three pilot clients.

## Blocked on

These need the maintainer or an outside party; everything else is decided in
`docs/DECISIONS.md`.

| # | Needed | From | Blocks |
|---|---|---|---|
| B-1 | Three reachable pilot clients; the one local-service category is chosen from them (D-026) | the maintainer, then the clients | the component-set half of I1 and everything client-facing after it; the I6 ship gate |
| B-2 | The model provider for real runs and a spending cap (D-018) | the maintainer | I2's real-provider pass (CI and the brief battery use recorded responses and are not blocked) |
| B-3 | Lead-form privacy notice text and a data-processing agreement with each pilot client (D-019) | the maintainer with each client | the first real lead (I6) |
| B-4 | Deploy access for each pilot site: the client's own Cloudflare account and token, or the operator-account fallback (D-017) | the maintainer with each client | promoting a client site (I6); I3's drills use an operator-owned test project |

## What exists (I0)

- `POST /v1/sites/{site}/leads` — a lead is acknowledged (`201`) only after
  it and its notification outbox row committed together; a resubmission
  with the same `submission_id` gets the same lead (`200`); everything else
  is a named refusal, and a `503` with `Retry-After` whenever nothing could
  be stored. Body cap, honeypot, per-address and per-site limits (D-019).
- A leased notifier that retries on the D-030 schedule on its own
  connection pool, so a dead or hanging notification provider never loses
  or delays a stored lead.
- The operator inbox at `/workspace` (SvelteKit + Tailwind v4 +
  shadcn-svelte), behind an argon2id password session.
- Drills against the real binary: provider down, provider never
  recovering, a lost answer after acceptance, `kill -9` mid-request, and
  PostgreSQL stopped mid-run — each ending in a log-capture check for lead
  content and secrets. `docs/modules/01-lead-intake.md` maps every claim to
  its test.

```sh
export PATH="$(brew --prefix rustup)/bin:$PATH"   # the pinned toolchain
scripts/stack.sh up            # PostgreSQL on 127.0.0.1:59432 (compose project intent-studio)
scripts/check.sh               # what CI runs: fmt, clippy, tests, architecture, drills, browser test
scripts/load.sh realism        # k6 with the provider down; results in perf/results/
```

## Documents

- `docs/END_GOAL.md` — what done means; the invariants; the four required
  failure cases and the security cases, each with its test and pass
  condition; the ship gate in testable form; the evidence plan
- `docs/ARCHITECTURE.md` — sources of truth, components, paths, egress and
  isolation, actors, consistency
- `docs/ROADMAP.md` — I0 spine → I1 brief/facts/components → I2 preview →
  I3 launch → I4 review → I5 evidence battery → I6 pilot and ship gate
- `docs/DECISIONS.md` — the guide's directions (D-001–D-009), the decisions
  made before code (D-010–D-029), those made while building I0
  (D-030–D-037) and the questions still open
- `docs/modules/01-lead-intake.md` — the lead-intake spine as built, and
  the test behind each claim

Contract: the engineering guide `06_Intent_Studio.docx` (kept in the
maintainer's workspace under `docs/Project_Guides/`, not in this repository;
extract with `textutil -convert txt -stdout <file>`).

**What counts as done:** the ship gate in `docs/END_GOAL.md` — each pilot
client signs off a live release, each pilot site receives a real inquiry,
each client can say what changed and why from the change log — with every
failure case passing on the release commit and the load evidence committed
under `perf/results/` (machine + commit on every result).
