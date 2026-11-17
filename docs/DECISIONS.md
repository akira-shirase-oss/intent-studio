# Decisions

Deliberate choices, newest last. D-NNN numbering follows the workspace
convention. D-001–D-009 seed the contract's prescribed directions;
D-010–D-029 were decided before any code to close the questions that
blocked milestone 0 and to fix gaps a plan review found; D-030–D-037 were
made while building I0. Where something
still needs the maintainer or an outside party, it is listed under *Open
questions (still open)* and in the README's *Blocked on* table.

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
  is containment. *Restated by D-011: no generated code runs (the page tree
  is typed); what needs isolation is untrusted input processing and
  Chromium.*
- **D-008 — No-change is a first-class review outcome.** The weekly review
  may recommend nothing; weak evidence produces a named hypothesis or
  no-change, never an auto-published optimization. The cost is a quieter
  changelog; the gain is honesty about what the evidence supports — and the
  guide's fourth failure case stays passable.
- **D-009 — Depth over breadth for the pilot.** One local-service category,
  one quote-request pattern, three reachable clients. The cost is a product
  that looks small; the gain is tested failure behavior instead of a gallery
  of untested templates.


## Decisions made before code (2026-11-16)

Each states the choice, the reason and what it costs. Numbers are proposed
starting values; changing one needs a measurement and a new entry.

- **D-010 — Approval binds the built bundle hash; promotion deploys stored
  bytes.** A candidate is the page-tree hash, the component-set version, the
  SHA-256 of the built, content-addressed bundle and the check record for
  that bundle. Approval records the bundle hash; promotion uploads the
  stored bytes and never rebuilds. Reason: if approval bound the page tree
  and the bundle were rebuilt afterwards, dependency or component drift
  would make the deployed bytes differ from the checked and approved bytes.
  Cost: bundles are stored per candidate (disk), and any component fix
  needs a new candidate and a new approval.
- **D-011 — What the worker isolates, and how.** The page tree is typed, so
  no generated code runs. Isolation is for untrusted input processing —
  image, SVG and font decoding, reference HTML parsing — and for Chromium
  loading the built page. Each job runs in a rootless container with a
  read-only root filesystem, no credentials, CPU, memory and wall-time
  limits. The render and check job has no network beyond the local server
  for the bundle under test (bundles are self-contained); import jobs reach
  out only through `intent-egress`. SVG uploads are rasterized, or
  sanitized to a strict element allowlist, before they can enter a bundle.
  Reason: this is what an attacker controls. A microVM is stronger but costs
  more to operate on one small host than the pilot justifies. Cost:
  container orchestration and image upkeep.
- **D-012 — One egress guard for every client-named URL.** Refused before
  connecting: loopback, RFC 1918, link-local (including 169.254.169.254),
  100.64.0.0/10, 0.0.0.0/8, multicast and reserved ranges, IPv6 loopback,
  link-local `fe80::/10`, unique-local `fc00::/7`, and IPv4-mapped forms of
  all of these. The DNS answer is validated and pinned for the connection
  (no second lookup, so no rebinding); at most 3 redirects, each
  revalidated; 10 s deadline; 5 MiB for HTML, 20 MiB for an asset. The
  guard has no "off" or "loopback is fine" mode; tests use a fixture
  resolver instead. Reason: reference import is a server-side request
  forgery path into cloud metadata and private services. Cost: some odd but
  legitimate references fail and must be uploaded as files.
- **D-013 — Every factual claim is a fact record, and facts expire.** Fact
  kinds: testimonial, metric, price, guarantee, licence, certification,
  insurance, service area, opening hours, contact details, legal promise.
  Each record has a source, an approver, `approved_at` and `review_by`
  (default 90 days; 30 days for prices and opening hours). A deterministic
  lint over free-text slots flags numbers, currency, percentages, years and
  a phrase list ("licensed", "insured", "certified", "guarantee",
  "warranty", "#1", "best"); a flagged phrase must be bound to a fact or
  removed, or the check record fails with `unbound_claim`. A fact past
  `review_by` blocks promotion (`fact_stale`), and live sites using it are
  flagged in the inbox for re-confirmation, not taken down. Reason: prices,
  licences and guarantees carry the same liability as testimonials, and the
  guide asks for approved facts to stay current. Cost: the lint has false
  positives that cost the operator a re-word; it is a second line behind
  human approval, not a guarantee.
- **D-014 — Generated links only point at allowlisted targets.** Links in a
  page tree may target the client's own domains, profile URLs recorded as
  facts, and `tel:`/`mailto:` values taken from contact facts. Anything else
  fails validation. Reason: a typed tree stops script injection, not link
  injection from a hostile reference page. Cost: an operator adds a fact to
  link anywhere new.
- **D-015 — Tenant, actors and approval links.** A tenant is one client
  business. The operator (the maintainer) is cross-tenant, signs in with a
  password session (argon2id hash; no magic-link login), and every
  cross-tenant action is audited. Client approvers have no accounts in the
  pilot: a single-use approval link, 48 h TTL, bound to the approver contact
  and the bundle hash, stored as a SHA-256 digest, consumed atomically, and
  never logged. Opening the link shows the preview and a confirm button;
  only the `POST` approves, so a mail scanner's prefetch cannot. Reason:
  fewer accounts and less auth surface for three clients. Cost: weaker
  identity than a login — anyone holding an unused link can approve; the
  TTL and single use bound that.
- **D-016 — A local content-addressed store for the pilot.** Assets and
  bundles live on the local filesystem under SHA-256 paths, behind a storage
  port, and are backed up with the Postgres dump; an S3-compatible backend
  is a later adapter. Reason: no extra service to operate for three clients.
  Cost: single-disk durability until backups are copied off the host; the
  restore drill covers both stores together.
- **D-017 — First deploy adapter: Cloudflare Pages direct upload.** The
  guide allows "GitHub/Vercel or alternative". Direct upload sends a
  prebuilt bundle, which is exactly what D-010 needs (the target never
  builds), and the maintainer already deploys this way. A `local-static`
  adapter serves bundles locally for CI and drills. Tokens are encrypted at
  rest with a key from the server environment and decrypted only inside
  `intent-publish`. Pages permissions in Cloudflare API tokens are granted
  per account rather than per project at the time of writing (verify at
  I3), so the preferred setup is the client's own Cloudflare account and a
  token the client can revoke; the fallback, one operator account, is
  recorded per tenant as reduced isolation. Cost: not the guide's named
  first example, and per-client accounts add onboarding work.
- **D-018 — One model provider behind the port; recorded responses
  everywhere else.** CI, the drills and the 50-brief battery run on recorded
  responses, so they are reproducible and cost nothing. Keys live only in
  the server environment or a secret file, never in a worker or a log. Each
  run carries a money budget. Which provider, and the monthly spending cap,
  are the maintainer's choice (still open, B-2). Cost: recorded responses
  go stale; they are re-recorded when the prompt version changes.
- **D-019 — Lead intake protections, retention and qualification.** Body cap
  16 KiB; a honeypot field; per-site 60 leads/min and per-IP 5 leads/min
  token buckets in a bounded table (100,000 entries, least-recently-used
  eviction); the client IP comes from the peer address or from
  `X-Forwarded-For` only when the peer is a configured proxy. The form
  carries a `submission_id` generated in the page; without script the
  server derives it from the site, the normalized payload and a 10-minute
  window. Notification emails are plain text, carry no submitted value in
  a header, and become an hourly digest above 10 notifications per hour per
  site. CSV exports prefix any cell starting with `=`, `+`, `-`, `@`, tab
  or carriage return with `'`. Retention: 12 months by default,
  configurable per tenant, with deletion on request (the lead and its
  outbox rows). The form links to a privacy notice; its text needs each
  client's agreement (open, B-3). Qualification is rule-based (service area,
  service type); no AI scoring in v1. Cost: some real leads from shared
  networks may hit the per-IP limit; the limit's refusals are counted.
- **D-020 — Notification retry schedule.** One attempt has a 10 s deadline;
  at most 8 attempts with exponential backoff and full jitter, starting at
  1 min and capped at 6 h (about a day in total); then the row is `dead`,
  shown in the inbox with a manual resend. The outbox event ID is sent as
  the provider's idempotency key where the provider supports one. Delivery
  is at least once; a duplicate email after an uncertain attempt is
  possible and is counted. Cost: a provider outage longer than a day needs
  a manual resend.
- **D-021 — Generation limits and failure taxonomy.** Starting caps per
  run: 3 repair iterations, 10 min wall time, 200,000 tokens, US$2, 256 KiB
  of tool output per call. Failure codes: `model_timeout`,
  `model_unavailable`, `model_invalid_output`, `budget_exhausted`,
  `render_crashed`, `checks_failed`, `reference_refused`. Only
  `model_timeout`, `model_unavailable` and `render_crashed` retry
  automatically, at most twice; a manual retry starts a new bounded job.
  The workspace shows each code with a plain explanation. Reason: the guide
  asks for a bounded retry and an explanation; undefined retries become
  retry-forever. Cost: some runs stop that one more iteration would have
  fixed; the battery measures how many.
- **D-022 — The promotion check suite defines "breaks".** Run against the
  stored bundle at 360×800, 390×844 and 1280×800 in Chromium and WebKit: no
  horizontal overflow (document scroll width ≤ viewport width); the form
  submits to a local `intent-server` and shows success only after the 201;
  required-field errors are announced (`aria-invalid`, `aria-describedby`,
  a live region); zero critical or serious axe-core violations; tap targets
  at least 24×24 CSS px (WCAG 2.2 target size, minimum); every interactive
  element reachable by Tab with a visible focus indicator and the form
  completable by keyboard alone; visual snapshots compared for review, but
  never sufficient on their own. Every result is stored with the bundle
  hash; `approve` and `promote` both require a passing record for that
  exact hash. Reason: the guide's failure case 2 had no milestone gate and
  "breaks" had no definition. Cost: a slower candidate cycle (three
  viewports, two engines).
- **D-023 — A pre-registered weekly decision rule.** The rule is versioned
  data (`review-rule v1`) and is committed before any pilot data exists. It
  compares the current release period with the previous one; an observed
  difference in lead rate (leads ÷ form views) is reported only when each
  period has at least 4 weeks and at least 30 leads and the 95% interval
  for the difference excludes zero. Otherwise the output is
  `insufficient_evidence` with `no_change` or a named hypothesis (the
  statement, the observation that would confirm it, the earliest re-check
  date). Traffic changes are reported separately and never as conversion
  effects. Every output is a queued record for human review. Reason: "weak"
  needs a rule fixed before the data, or it becomes whatever looks good.
  Cost: most pilot sites will honestly report insufficient evidence for
  months.
- **D-024 — Authorized observations are cookieless aggregate counts.** A
  first-party beacon to `intent-server` counts page loads, form views and
  leads per site, release and day. No cookies, no stored IP or user agent,
  no per-visitor rows, so there is no personal data and no consent banner.
  Reason: matches the low-volume honesty rule and keeps the pilot clear of
  tracking law. Cost: no attribution by source, which the guide's full
  direction ("source-aware lead analytics") would need its own contract
  for.
- **D-025 — Load units: lead submissions and beacon events, never a third
  party.** The 1,000,000-unit runs count lead submissions (I0 smoke, I3
  full) and beacon events (I4) against `intent-server` on the test machine.
  Published pages are served by the deploy target, so a million-request run
  against them would load-test Cloudflare, not this project; pages are
  measured locally with Lighthouse budgets instead. The model, email and
  deploy providers receive only functional traffic. Reason: the workspace
  rule allows the equivalent unit, and testing someone else's service
  breaks their terms and measures nothing of ours.
- **D-026 — Milestone 0 is the lead-intake spine; the category waits for
  the clients.** I0 builds CI, lead intake, the outbox, the notifier and
  the provider-down and database-down drills — everything that does not
  depend on the category. The category is chosen from the three clients the
  maintainer can actually reach (B-1); building the component set first
  would be the "many untested templates" failure the guide warns against.
  The roadmap gains I0 (spine), I5 (evidence battery) and I6 (pilot and
  ship gate), and a status cell says *Done* only with named tests and an
  evidence path. Cost: the visible product (generated sites) arrives later
  than the plumbing.
- **D-027 — Previews are served by `intent-server`, not the deploy target.**
  A preview is the stored bundle served behind the operator session or an
  approval link, with `X-Robots-Tag: noindex` and a `noindex` meta tag;
  forms in a preview post as test leads, which are stored with a test flag
  and never notify the client. Only promotion uploads to the target.
  Reason: previews carry unapproved claims, and hosted preview URLs are
  often public. Cost: preview traffic is served by the operator host.
- **D-028 — Health monitoring is out of scope, not open.** The guide puts
  frequent health monitoring under the full direction. It blocks nothing in
  the first release; a client who wants it can use any external monitor,
  and nothing in this project depends on one.
- **D-029 — No secrets or lead PII in logs, proven by a test.** A
  log-capture test runs over every drill and fails on known secret shapes
  (API keys, bearer tokens, approval-link and deploy tokens, provider keys)
  and on the fixture leads' emails and phone numbers. Logs carry lead IDs,
  not lead content. Cost: debugging a lead means reading the inbox, not the
  log.

## Decisions made while building I0 (2026-11-16)

- **D-030 — D-020's retry arithmetic, corrected.** D-020 promised "about a
  day in total" from a 1-minute start, a 6-hour cap and 8 attempts with
  full jitter, but doubling from 1 minute reaches only 64 minutes by the
  seventh wait (at most 127 minutes in total, about an hour on average
  with full jitter), so a provider outage of a couple of hours would have
  killed most notifications. The schedule now grows 4× per attempt: after
  failed attempt *k* the wait is drawn uniformly from `[c/2, c]` with
  `c = min(6 h, 1 min × 4^(k−1))` — ceilings 1, 4, 16, 64, 256, 360 and
  360 minutes, 8.8 to 17.7 hours of waiting in total. The floor (equal
  jitter instead of full jitter) is what makes the total predictable;
  spreading stays wide (half the ceiling). Base, cap and attempts are
  unchanged; drills and load runs scale the base (the cap is always
  360 × base). Proven by `intent-domain` `outbox` tests and the
  `notify-down` drill's per-key schedule check. Cost: an outage longer
  than about 9 hours can still exhaust attempts; the row is `dead`,
  visible, and keeps its lead.
- **D-031 — Lead fields, consent and retention days.** A lead is name
  (required, ≤ 200 characters), e-mail (≤ 254, shape-checked) and/or phone
  (6–15 digits) — at least one contact — an optional message (≤ 5,000),
  service and area (≤ 100 each), and `consent`, which must be `true` (a
  ticked box: the visitor agrees to be contacted under the site's privacy
  notice, whose text is still B-3). Unknown fields are refused, so stored
  content is exactly what the form declares. Retention is counted in days
  (365 by default, 30–3,650 per tenant), so "12 months" never depends on
  month lengths; each lead stores its own `retain_until`. A filled
  honeypot is refused with `400 rejected` — honest, but deliberately
  unspecific — rather than answered with a fake success, because nothing
  may be acknowledged that was not stored. Validation errors name the
  field and a code and never echo a value.
- **D-032 — `intent-notify`, and who may hold an HTTP client.** The
  notification provider port and its adapters live in their own crate:
  `intent-notify` (a webhook adapter speaking JSON with the outbox row id
  as `Idempotency-Key`, and an in-memory recorder). The architecture
  check's HTTP-client rule (A3) allows an HTTP client crate only in
  `intent-egress` (client-named URLs), `intent-agent` (the model port)
  and `intent-notify` (the operator-configured provider endpoint, never
  a URL from client input). In I0 the webhook adapter speaks plain HTTP
  only, to the local provider fixture; TLS (rustls) arrives with the
  first real e-mail adapter at I3. `intent-provider-fixture` is the
  provider-down harness (ok, 503, hang, accept-then-hang), development and
  test only.
- **D-033 — Deferred to I3, with the first release: CORS, the no-script
  303, the hourly digest, manual resend, retention purge.** Each of these
  needs something I0 does not have — a site's release origins and
  thank-you page (CORS, 303), the real e-mail adapter (digest above 10 an
  hour per site, D-019), the inbox's write actions (manual resend of a
  `dead` row, D-020), the deletion path (purge past `retain_until`).
  I0 accepts JSON and form posts and answers both with JSON; no published
  site can post to it until I3 anyway.
- **D-034 — The notifier never shares intake's resources.** It has its
  own connection pool (4 by default) and a bounded number of attempts in
  flight (32), claims only as many rows as it can start, and holds no
  transaction across a provider call. Measured reason: in the first
  `notify-down` drill, with one shared pool, intake p99 rose with the
  notifier's backlog. After an attempt the notifier keeps retrying the
  record while its lease can still be valid (lease − attempt deadline),
  so a short database blip does not turn into a duplicate notification;
  longer than that, the lease expires and the row is sent again, counted
  as an uncertain attempt. Every drill asserts duplicates ≤ uncertain
  attempts.
- **D-035 — Readiness and refusals while the database is down.** A
  prober pings PostgreSQL every 500 ms (1 s deadline); readiness is red
  while the last probe failed, and intake answers `503 unavailable` with
  `Retry-After: 2` at once instead of queueing. Storing a lead has a
  3-second deadline and the pool a 1-second acquire timeout; past either,
  the answer is a 503 that acknowledges nothing (a commit that lands after
  a timeout is found by the resend, which gets `200` with the same lead).
  Proven by the `db-down` drill.
- **D-036 — The operator session.** `POST /v1/operator/session` verifies
  the argon2id hash from `INTENT_OPERATOR_PASSWORD_HASH` off the async
  workers (two at a time, five attempts a minute per address); the
  session token is 32 random bytes in an `HttpOnly`, `SameSite=Strict`
  cookie with a 12-hour lifetime, stored only as a SHA-256 digest. Every
  inbox page read is cross-tenant and writes an `operator_audit` row with
  ids and counts, never content. Without a configured hash, sign-in is
  off (`503`), not open.
- **D-037 — Drills run the release binary; load runs scale the retry
  schedule.** The drills (`scripts/drills.sh`) test the release build,
  as deployed: the debug build's latency under the drills' load measured
  ten times worse and said nothing about the product. Load runs set the
  retry base to 1 second (the D-030 schedule at 1/60 scale), so a run sees
  every attempt, the drain after recovery and the `dead` cases in minutes;
  the production base stays one minute. Each load run gets a database of its
  own and drops it afterwards, and its pass condition cross-checks the
  outbox against the provider (rows marked delivered = distinct provider
  acceptances; duplicates ≤ uncertain attempts): a stray server left over
  from a manual test once reconnected to a reused database name and marked
  rows delivered through its own recorder, which only that cross-check
  could have caught. The run is kept in `perf/results/`, marked invalid.

- **D-038 — Connection bounds, and no body read before the cheap
  refusals.** `axum::serve` exposes no connection settings, so the server
  drives hyper's HTTP/1 connection itself: at most 1,024 connections open
  (a permit is taken before `accept`, so excess connections wait in the
  kernel backlog instead of costing a descriptor and a task); a 10 s
  header-read timeout that also closes idle keep-alive sockets (hyper's
  timer runs whenever a connection waits for a request head); a 10 s
  write-stall deadline for clients that stop reading. Intake and operator
  sign-in take the body unread and read it (16 KiB cap, request timeout)
  only after the shed gate and the rate limits, so a refused client never
  makes the server wait for its bytes. Proven by
  `connections::slow_headers_and_idle_sockets_are_closed`,
  `open_connections_are_capped`,
  `a_client_over_its_limit_is_refused_before_its_body_is_read` and
  `a_client_that_stops_reading_is_cut_off`, each failing before the
  change. Cost: a client may keep a connection idle for at most 10 s.

- **D-039 — What spends a site's budget.** The per-site bucket (60 a
  minute, D-019) is charged only for a lead that validated and whose site
  accepts leads, and a replay or a conflict refunds its token. The site is
  checked against a bounded cache of sites verified to accept leads (a
  miss costs one primary-key lookup; unknown and closed sites are never
  cached), so a made-up site id is a 404 that creates no rate-limit row.
  Reason: charging before validation let a bot's honeypot posts lock a
  real site's visitors out, and random site ids grew the table. Proven by
  `intake::only_storable_leads_from_known_sites_spend_the_site_budget`,
  which failed before the change. The per-address bucket still charges
  every request: that is the one a flood has to get through first.

## Open questions (still open)

These need the maintainer or an outside party; each is in the README's
*Blocked on* table.

- **Pilot clients and the category (B-1).** Who the three reachable clients
  are; the category follows from them (D-026).
- **Model provider and spending cap (B-2).** The provider for real runs and
  the monthly budget (D-018).
- **Privacy notice and data processing (B-3).** The notice text on each
  client's form and the agreement under which the operator processes their
  customers' leads (D-019); counsel if a client asks for it.
- **Deploy access per client (B-4).** The client's own Cloudflare account
  or the operator-account fallback (D-017).
