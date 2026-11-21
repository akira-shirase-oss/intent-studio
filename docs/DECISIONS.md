# Decisions

Deliberate choices, newest last. D-NNN numbering follows the workspace
convention. D-001–D-009 seed the contract's prescribed directions;
D-010–D-029 were decided before any code to close the questions that
blocked milestone 0 and to fix gaps a plan review found; D-030–D-053 were
made while building I0, and D-054 onward while closing I0 and building I1.
Where something
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

- **D-040 — Outbox records are fenced per claim.** A notifier records an
  attempt's result only if the row still has the lease owner *and* the
  attempt number it claimed. The owner alone (one id per process) let a
  late result from an expired claim be applied over the same process's
  newer claim, which then lost its own result and left the row pending
  with no attempt left and no lease — neither claimable nor sweepable.
  The sweep now also marks dead any pending row with no attempt left that
  nobody holds (no lease, or an expired one), using a partial index.
  Proven by `outbox_fencing::a_late_result_from_an_expired_claim_cannot_overwrite_the_current_one`
  and `a_row_with_no_attempt_left_and_no_lease_is_swept_dead`, both
  failing before the change.

- **D-041 — Operator sign-in, hardened.** On top of D-036: a global
  budget of 20 sign-in attempts a minute from all addresses together (so
  guesses spread over many addresses are bounded; the cost is that during
  such an attack the operator may wait a minute); an audit row for every
  failed attempt as well as every success and inbox read, each with the
  actor (`operator` or `unknown`), the client address and the request id;
  the session cookie named `__Host-intent_operator` whenever it is
  `Secure` (browsers then refuse it unless Secure, host-only and for `/`;
  plain-HTTP development servers keep the unprefixed name, which a browser
  would otherwise drop); and a 30-minute idle timeout inside the 12-hour
  lifetime, the session checked and touched in one statement. Proven by
  `operator::sign_in_is_audited_uses_a_host_cookie_and_expires_when_idle`
  and `sign_in_has_a_global_budget_beyond_the_per_address_one`, both
  failing before the change.

- **D-042 — No bidirectional controls or invisible characters in a
  lead.** Every lead field refuses Unicode bidirectional controls
  (U+202A–U+202E, U+2066–U+2069, U+200E, U+200F, U+061C) and invisible
  characters (U+200B, U+200C, U+2060–U+2064, U+FEFF, U+180E) with
  `invalid_characters`: an operator reading the inbox, or a client reading
  the notification, must see what was typed. The zero-width joiner is
  refused everywhere except the message, where emoji sequences need it.
  Proven by `lead::tests::bidi_controls_and_invisible_characters_are_refused`,
  which failed before the change.

- **D-043 — The workspace's security headers.** Every response carries
  `Content-Security-Policy: frame-ancestors 'none'; base-uri 'none';
  object-src 'none'; form-action 'self'`, `X-Frame-Options: DENY` and
  `Referrer-Policy: no-referrer`; the workspace's pages add their own
  policy in a meta tag, built by SvelteKit, that pins every inline script
  by hash (`default-src 'self'`; fonts may also be `data:`, which Vite uses
  for small subsets). Proven in Chromium by
  `the workspace runs under a content security policy and cannot be
  framed` (`scripts/workspace-e2e.sh`): the headers, the hashed script
  policy, the app booting with zero violations, a deep link under the same
  rules. It failed before the change.

- **D-044 — A lead records the notice it consented under.** The form
  sends `consent_notice`, the version id of the privacy notice it showed
  (1–64 characters of `A-Z a-z 0-9 . _ : -`; the text itself is still
  B-3), and the lead stores it beside `consent`. It is required: consent
  without the notice it was given under proves little, and the site forms
  this project generates always know the version. Leads stored before the
  column existed read `unrecorded`. Proven by
  `intake::the_consent_notice_version_is_required_and_stored_with_the_lead`
  (it failed before the change: the field was refused as unknown) and the
  domain's validation tests.

- **D-045 — A development profile, and the recorder only in it.**
  `INTENT_PROFILE` is `production` (the default) or `development`. The
  in-memory `recorder` provider accepts and forgets every notification, so
  in production it would be a provider that silently delivers nothing: it
  is refused unless the profile is `development`. Proven by
  `config::tests::the_recorder_is_refused_outside_development`, which
  failed before the change.

- **D-046 — Which provider answers end a notification.** Only a refusal
  of the notification itself — 400, 413, 415, 422 — makes the row `dead`
  at once. 401, 403, 404 and any 3xx (redirects are not followed) mean our
  configuration is wrong; the operator can fix it, so they are retried on
  the D-030 schedule like 408, 429 and 5xx, instead of killing every
  notification on its first attempt. Proven by
  `intent-notify` `statuses_map_to_outcomes`, which failed before the
  change (301 and 401 were permanent).

- **D-047 — Readiness probes with a connection of its own.** The
  database prober (D-035) has a one-connection pool instead of borrowing
  intake's. Borrowing, a saturated intake pool (a burst, or a lock on
  `leads`) made the probe time out, readiness go red, and intake refuse
  everything as "database down" while the database was fine — a busy
  server turning itself off. Proven by
  `readiness::readiness_stays_green_while_intakes_pool_is_saturated`
  (red 18 times in 3 s before the change, 0 after).

- **D-048 — The notifier's pool fits its concurrency.** D-034's pool of
  4 connections served 32 concurrent attempts with intake's 1-second
  acquire timeout; in the 100,000-submission run, recording results timed
  out waiting for a connection. The notifier pool now defaults to 8
  connections with a 5-second acquire timeout (a background record can
  wait where a visitor should not), and a configuration with more than 8
  attempts in flight per notifier connection is refused. Proven by
  `config::tests::the_notifier_pool_is_sized_for_its_concurrency`, which
  failed before the change.

- **D-049 — Who may read `/metrics`.** With `INTENT_METRICS_TOKEN` set
  (16 characters or more), only `Authorization: Bearer <token>` reads it;
  without one, only a direct request from the host itself (a loopback
  peer with no `X-Forwarded-For`, so a local reverse proxy cannot turn it
  into a public page). Everyone else gets a 404. The counters are not
  personal data, but refusal and rate-limit counts tell an attacker how
  the limits respond. Proven by
  `intake::metrics_are_for_the_host_or_a_token_holder_only`, which failed
  before the change.

- **D-050 — A floor for the operator's password hash.** At startup the
  configured hash must parse as a PHC string, be argon2id, and use at
  least 19 MiB of memory and 2 iterations (OWASP's baseline, which
  `intent-server hash-password` meets); anything else refuses to start,
  without quoting the hash. Proven by
  `config::tests::a_weak_or_malformed_password_hash_is_refused`, which
  failed before the change (a 8 KiB, 1-iteration hash loaded).

- **D-051 — Bounds per connection and per address.** On top of D-038: a
  connection serves at most 1,000 requests (the last response carries
  `Connection: close`) and lives at most 5 minutes (then it finishes its
  request in flight and closes); one client address — an IPv6 /64 — holds
  at most 64 connections (never more than the global cap), and further
  connections from it are closed at accept. A trusted proxy is exempt: it
  carries many clients, which the per-address request limits tell apart
  by `X-Forwarded-For`. Reason: one connection answered 500 pipelined
  requests and stayed 40 s, and 20 keep-alive sockets from one address
  pinging every 8 s held a cap of 20 for 33 s. Proven by
  `connections::a_connection_serves_a_bounded_number_of_requests`,
  `a_busy_connection_is_closed_at_its_maximum_age` and
  `one_address_cannot_hold_every_connection`, all failing before the
  change.

- **D-052 — The global sign-in budget cannot be turned against the
  operator.** D-041's two checks ran eagerly, so requests refused per
  address (or for their content type) still drained the global budget,
  and 25 junk posts from one address locked the operator out. Now the
  per-address check runs alone first; a global token is taken only right
  before a password is verified and is given back when it is right, so the
  global budget counts failed verifications; and an address the operator
  signed in from successfully in the last 30 days (the audit table) skips
  the global budget, so a distributed flood cannot keep the operator out
  of their usual place. The cost: from an address never used before, the
  operator can still be held up by a distributed flood. The admission
  rules take time as a parameter and are unit-tested with exact counts.
  Proven by `operator::a_flood_from_one_address_cannot_lock_the_operator_out`
  and `the_operators_known_address_gets_through_a_distributed_flood`
  (both failed before the change) and the `SignInGate` unit tests.

- **D-053 — The per-address connection default is a quarter of the cap.**
  `INTENT_MAX_CONNECTIONS_PER_ADDRESS` defaults to `min(64, max(1, cap/4))`
  instead of `min(64, cap)`: at a small cap (20) the old default let one
  address hold every connection and lock newcomers out, which D-051 was
  meant to prevent. Proven by
  `config::tests::one_address_gets_at_most_a_quarter_of_a_small_cap`, which
  failed before the change (20 of 20). What this does not cover: at the
  default cap of 1,024, 16 IPv6 /64s fill every connection, and a single
  subscriber often holds a /56 or /48 (256 to 65,536 /64s), so the
  per-address limit alone does not stop one determined IPv6 client from
  taking the cap; its connections still close at the 10 s idle timeout and
  the 5-minute age (D-038, D-051), and each serves at most 1,000 requests.
  Kept for now, unmeasured: keying the connection count by a wider IPv6
  prefix (/48) would also group unrelated users behind one provider's
  allocation, and lowering 64 would hurt clients behind carrier-grade
  NAT. Listed under the open questions until a measurement decides it.

## Decisions made while closing I0 and building I1 (2026-11-17)

- **D-054 — The intake budget, set from the quiet-machine baselines.** Failure
  case 3 asks for "intake p99 within the budget set from the I0 baseline";
  until now the 100 ms p99 in every run was a proposal. The baselines, each
  started below load average 6 (`perf/results/README.md`): realism (50/s)
  p99 14.7 ms at `bd8d4e6`; burst (250/s) p99 89.9 ms at `c405dce`, logged
  at debug while other builds pushed the load to 14.75 before it ended;
  `notify-down` (100,000 at 500/s) p99 5.4 ms and the full run (1,000,000
  at 500/s) p99 12.1 ms, p99.9 88 ms, both at `e61d0c2`. The budgets
  (`scripts/load.sh` sets them per scenario; `perf/k6/leads.js` enforces
  them as thresholds):

  | Scenario | p99 | p99.9 | Headroom over the baseline |
  |---|---|---|---|
  | realism, 50/s | 50 ms | — | 3.4× the 14.7 ms p99 |
  | burst, 250/s | 200 ms | — | 2.2× the 89.9 ms p99 |
  | `notify-down` and full, 500/s, provider down | 100 ms | 250 ms | 8.3× the 12.1 ms p99; 2.8× the 88 ms p99.9 |

  The other pass conditions are unchanged: acknowledged rate above 99.9%,
  dropped iterations below 0.01% of the run, zero reconciliation
  violations, zero log lines with lead content, outbox = provider. Why
  these numbers: the budget's job is to catch a regression, not to
  flatter a laptop. The busy-machine misses it must catch were 109 ms to
  402 ms at p99 (pool starvation and scheduler contention), and the quiet
  runs vary by about 6× between scenarios, so a budget at 2–3× the worst
  quiet baseline of each scenario flags those failures without failing on
  ordinary laptop noise. Burst gets the widest absolute allowance because
  its baseline is the noisiest (debug logging, load rising mid-run) and it
  deliberately queues at 5× the guide's rate; it keeps at least 2×. A
  visitor does not perceive any of these: the budget is a regression
  line, not a user-experience target. Every recorded quiet run already
  meets its budget, so no run was repeated to pass it. Cost: a budget at
  2–3× a laptop baseline says nothing about production capacity — the
  results stay development observations (D-025), and a production host is
  measured on its own.

- **D-055 — I1 is built in its smallest gate-meeting form.** The row names
  a schema, fact records, the page-tree spec and validator, the claim
  lint, the content-addressed store, `intent-egress`, the asset worker and
  reference notes, then (blocked, B-1) the component set. What that means
  here: facts and their approvals, briefs with revisions, and a
  `spec-check` endpoint that validates a page tree against a brief and the
  tenant's facts — storing page trees is I2's job, with candidates; the
  reference notes are the brief's references (address, permission, notes)
  plus a snapshot fetched through the egress guard into the store, whose
  bytes are kept and not parsed (parsing is the worker's, at I2); the
  asset worker sanitizes SVG and decodes PNG, JPEG and TrueType/OpenType
  fonts (D-062); the operator does all of this in the workspace, which
  gains Facts, Briefs and Assets screens (D-063), and every operator
  action is audited (D-015). Out of I1: the generation loop, parsing
  references, WOFF2 and WebP, rendering components. The gate's second half
  — components rendering a spec deterministically at three viewports —
  stays blocked on B-1. Cost: the spec check proves the vocabulary and the
  rules before any page exists, so a rule may still change when the first
  component is built against it.

- **D-056 — Facts: immutable content, recorded approvals, calendar days.**
  A fact's kind, statement, source and contact link are written once; a
  corrected statement is a new fact, so a page that referenced a fact
  always meant what was approved. What changes is its approval: until
  approval links exist (I3, D-015) the operator records it on the
  client's behalf — who approved, the day they approved, and a review-by
  date at most 90 days later (30 for prices and opening hours, D-013),
  earlier if the operator chooses; recording a later approval
  re-confirms the fact. Approvals are calendar days in UTC, never in the
  future and at most ten years back; a fact is current through its
  review-by day and stale from the day after. A withdrawn fact stays on
  record and is never usable again. Only contact facts carry a link, and
  exactly one: a phone number (stored as dialled), an e-mail address, or
  an `https` address. Every write and its audit row commit in one
  transaction. Cost: a typo in an approved statement means a new fact
  and a new approval; a client in a time zone far from UTC sees review
  dates shift by a day at the edges.

- **D-057 — The brief.** Fields and bounds: business name (1–120),
  goal (`quote_requests`, the one pattern, D-009), intent and audience
  (1–1,000 each), 1–20 services (names unique ignoring case, 1–80;
  descriptions up to 500), brand (tone up to 200, up to six `#rrggbb`
  colours, an uploaded logo asset of the tenant), up to five client
  domains (host names, the only addresses pages may link to besides
  contact facts, D-014), up to ten references, and up to 50 approved
  claims. A reference is an `http` or `https` address, a permission —
  `inspiration_only` (look only), `own_content` (the client's own site:
  its text and images may be reused) or `licensed_assets` (images under
  the licence the notes must name) — and notes up to 2,000 characters.
  Claims must be approved, current facts of the same tenant when the
  brief is saved (and a database foreign key keeps them in the tenant);
  they are checked again whenever a page tree is. A brief is a sequence
  of revisions: a change names the revision it was made from and is
  refused with `revision_conflict` if another was saved first. The stored
  form is the validated brief's own JSON, re-validated whenever it is
  read back. Cost: stricter than a free-form intake — anything the
  schema has no field for goes into intent or notes.

- **D-058 — The page tree's vocabulary, `quote-request@0`.** Ten
  components: hero, services, testimonials, trust, service area, hours,
  contact, quote form, text and footer. Every slot is typed: headings and
  body text (plain, or bound to facts that support its claims), claim
  slots that hold only a fact id of the kinds the slot allows
  (testimonials take testimonials; trust takes metrics, licences,
  certifications, insurance and guarantees; prices, service areas, hours,
  contact details and legal promises each have their own slots), and
  link targets with four forms only — an anchor on the page, a page of
  the tree, a contact fact's link, or an `https` address on one of the
  brief's domains (no port, no credentials, exact host). There is no
  "any URL" and no HTML. A tree has 1–5 pages including `/`, 1–20
  sections each, and the home page must hold a quote form whose services
  are the brief's. A claim slot must name a fact the brief lists; a fact
  the brief does not list is `fact_not_in_brief` even when approved.
  Unknown components and fields fail parsing, reported by path. Cost: a
  page cannot say anything the vocabulary lacks a slot for; adding one is
  a new component-set version.

- **D-059 — `claim-lint v1`.** Flags every number, price (a number after
  a currency sign), percentage, year (1900–2099) and claim word — D-013's
  list (licensed, insured, certified, guarantee, warranty, #1, best) and,
  by prefix, licence/license, insurance, certification, accredited,
  award and approved — in every free-text slot, titles, descriptions and
  button labels included. A flagged token is supported only when a fact
  bound to that slot contains the same token (a number as a whole number:
  500 is not supported by 1500); otherwise it is `unbound_claim` with the
  token as its detail. When a bound fact itself is unusable, that is the
  reported problem and the lint is not run on the slot. Proven by
  `claims::tests` and `page_tree::tests::invented_claims_in_free_text_are_unbound`.
  Cost: plain copy with a number ("open 7 days") needs a bound fact or a
  re-word; the lint is versioned data, and changing it changes the
  version.

- **D-060 — The egress guard, built on its own HTTP/1.1 client.**
  `intent-egress` speaks HTTP/1.1 through hyper's client connection over
  a stream its `Connect` port opens, after its `Resolve` port answered
  and every address in the answer passed the policy. A general HTTP
  client (reqwest) resolves and connects internally, so the check and
  the connection could not be shown to use the same answer, and a test
  could not reach a fixture without allowing loopback; with the two
  ports, production uses the system resolver and TCP, and tests use a
  fixture resolver and an in-memory pipe to an in-process server — the
  policy never changes (D-012). The policy is an allowlist for IPv6
  (`2000::/3` minus its special blocks) and a denylist of every IPv4
  special-purpose block (documentation and benchmarking ranges included,
  beyond D-012's list); IPv4 inside IPv6 (mapped, NAT64, 6to4) is judged
  as IPv4. A name whose answer holds one forbidden address among public
  ones is refused outright. Only ports 80 and 443; `Accept-Encoding:
  identity` (no decompression bomb); `https` over rustls with the ring
  provider and the Mozilla roots compiled in (`webpki-roots`), never
  downgraded to `http` by a redirect; at most 3 redirects, each checked
  from the start; one 10 s deadline for the whole fetch; 5 MiB pages,
  20 MiB assets, refused on `Content-Length` before reading. Every
  refusal has a stable code and is recorded on the snapshot row. Proven
  by `intent-egress` `policy::tests` (every block at its edges, public
  neighbours pass) and `tests/guard.rs` (each refused before any
  connection; rebinding; redirects to private targets; bounds), and by
  the `assets` drill (a loopback listener sees zero connections). Cost:
  HTTP/2-only sites and references on other ports cannot be fetched;
  they can be saved and uploaded as files.

- **D-061 — The content-addressed store's layout and guarantees.**
  `intent-store` keeps blobs at `<root>/sha256/ab/cd/<hex>`, written to
  `<root>/tmp/` under a fresh name, flushed, renamed into place and the
  directory flushed, so a crash leaves the whole blob or nothing under
  its name; storing the same bytes again is a no-op; every read re-hashes
  and a mismatch is `Corrupt`, never served; at most 32 MiB a blob; every
  operation runs off the async workers under a 10 s deadline. The root is
  `INTENT_STORE_DIR`; a configured store that cannot be written stops
  startup. Without one, uploads and snapshots answer `503
  store_unavailable` and everything else works. Blobs whose database row
  never committed (a crash between storing and recording) are orphans:
  harmless, unreferenced, and left for a later sweep. Cost: one disk until
  backups are copied off the host (D-016); the operator guide's backup
  covers the database and this directory together.

  *Addendum 2026-11-20:* a root is single-store, enforced not assumed.
  `open` takes an exclusive advisory lock on `<root>/lock`, held for the
  store's life and released by the OS however the process ends — a second
  open on a held root is refused `Locked` before it can touch anything,
  because its `tmp/` sweep would otherwise eat the live holder's
  in-flight writes. `open` then sweeps `tmp/`: at startup nothing can be
  in flight. And "storing the same bytes again is a no-op" holds only
  while the stored bytes still hash to their name — a same-length torn
  file is rewritten, never trusted by its length.

- **D-062 — The asset worker: one process per file, now; the container
  at deployment.** D-011 puts untrusted decoding in a rootless container.
  I1 builds the part that does not depend on a container runtime and is
  testable everywhere: `intent-worker` is a separate binary — the only
  crate allowed a decoder (architecture rule A6) — started per file by
  `intent-render`'s driver with an empty environment (no database URL or
  key can reach it), stdin in, one framed answer out, stderr discarded,
  a 10 s wall-time kill, a cap on its output, and two jobs at once. Before
  reading a byte it limits itself: 10 s of CPU, no file writes, no core
  dumps, 16 descriptors and, on Linux, 1 GiB of address space; a limit it
  cannot set stops it. A crash, an abort, a signal or garbage on stdout is
  `worker_crashed` for that one upload; the server keeps serving. Formats:
  SVG is rebuilt from an element and attribute allowlist (no DTD, a node
  limit, only same-document references, every value escaped) — chosen
  over rasterizing because a logo must stay a vector and the allowlist
  writes nothing it does not name; PNG and JPEG are decoded under 8,192
  pixels a side and 256 MiB and re-encoded, which drops all metadata;
  TrueType and OpenType fonts are parsed and every glyph drawn with
  skrifa (ttf-parser is unmaintained, RUSTSEC-2026-0192) and stored
  unchanged. WebP, GIF, WOFF and WOFF2 are refused for now
  (`unsupported_type`). Uploads are `multipart/form-data` with a custom
  header a cross-site form cannot send. Still owed for D-011, at
  deployment: the container itself — read-only root, no network, no
  credentials mounted — around the same binary; the operator guide
  describes it. Proven by `intent-worker` `tests/assets.rs` (the hostile
  SVG keeps 0 of its 17 dangerous parts; DTD, mismatched and truncated
  files refused; metadata gone; limits read back from `/proc` on Linux),
  `intent-render`'s driver tests (signal, non-zero exit, hang, flood,
  absent binary, empty environment) and the `assets` drill (a worker
  that dies by `SIGSEGV` fails only its upload; readiness and intake stay
  up). Cost: a process per file costs a few milliseconds of start-up;
  the pilot's upload rate makes that irrelevant.

- **D-063 — The workspace's studio screens, and how "usable" is proven.**
  I1 adds Facts, Briefs (list, form, read with revisions and reference
  copies) and Assets to the inbox, under one shell with Leads, Facts,
  Briefs and Assets, a client picker, a light/dark/system choice and a
  skip link. The maintainer's interface standard, as built: only the UI
  kit's components and tokens; every screen has an empty state that says
  what to do next, skeletons the height of the rows they stand for, an
  error with plain words and a way to retry, a success notice, and an
  honest uncertain state — a write that got no answer, or a 503, says the
  save could not be confirmed and to check before retrying, never that it
  failed or succeeded; refusals are in plain words next to their field,
  summarized with links, with the server's codes behind a "details"
  toggle; forms open with focus inside them; the operator calls tenants
  "clients". Proven by `apps/workspace/tests/studio.spec.ts` and
  `inbox.spec.ts` at 1440 px and 360 px in Chromium: the three studio
  journeys (keyboard-only where it matters), axe with WCAG 2.0/2.1 A and
  AA rules showing zero serious or critical violations on all eleven
  screens in light and in dark, no sideways scrolling, and the CSP
  checks; review screenshots of each screen at both widths in both
  schemes are in `docs/screens/` (`scripts/screens.sh`). Long lists use
  `content-visibility: auto` (the browser skips rows off screen) under the
  server's 500-row cap, rather than a virtual-list library: for a pilot
  client's tens of facts it costs nothing and keeps native find, focus
  and screen-reader order. The bundle budget (`scripts/check-bundle.sh`,
  run by the gate): all JavaScript 160 KiB gzipped (115 measured), no
  chunk over 48 KiB (32), CSS 48 KiB (32). The studio API's load scenario
  is `perf/k6/studio.js` with proposed thresholds (reads p99 < 50 ms, the
  spec check p99 < 100 ms, no failures); it has not been run, and its
  budget is set from its first quiet baseline as D-054 did for intake.
  Uploads have no load scenario: two worker slots, refused with `503`
  past them, make it a capacity limit by design, not a hot path. Cost:
  page trees have no screen yet (the spec check is API-only until I2
  shows candidates), and Chromium is the only engine tested (WebKit
  arrives with the promotion suite, D-022).

  *Addendum 2026-11-20 — the thresholds are set.* The first baseline
  (`perf/results/2026-11-20-studio-baseline-50070ec.json`: 12,000
  requests at 200/s on an M2 Pro, load 5.6 falling from 31) measured
  reads p99 10.5 ms and the spec check p99 10.4 ms, with p99.9 ~108 ms.
  The gates are now p99 < 45 ms for both — four times the measurement,
  a margin chosen for CI's shared runners, which run loopback
  Rust+Postgres work roughly two to four times slower than a quiet
  laptop and carry more noise; a run that misses four-times-baseline
  has a real regression, not a slow runner. The p99.9 tail is recorded
  in every result but not gated: 108 ms was observed on a loaded
  laptop and gets judged on CI's dedicated `studio-load` run before it
  earns its own number.

- **D-064 — A tested production path.** The maintainer deploys this
  himself, so the way in is part of the product: `scripts/package.sh`
  builds a release package (both binaries, the built workspace, the
  systemd unit, the env example, the Caddyfile, the operator guide);
  `deploy/intent-studio.env.example` lists every setting with the
  decided defaults; `deploy/intent-studio.service` runs as its own user
  with a state directory and a sandbox (strict read-only system, no new
  privileges, a system-call filter that leaves the worker its
  `setrlimit`); `deploy/Caddyfile` terminates HTTPS and passes the
  client's address; `intent-server check-config` reports every
  configuration problem without starting; `docs/OPERATOR_GUIDE.md`
  covers install, first run, upgrade, backup and restore, and
  troubleshooting. `scripts/test-deploy.sh` (in the gate) installs the
  package into a scratch prefix, fills the env example, runs
  `check-config` (and proves it names a missing variable), migrates,
  serves in the production profile, signs in with the `__Host-` cookie,
  stores and notifies a lead, uploads through the real worker, backs up
  database-then-store, restores both into a fresh database and store and
  serves the same lead and the same asset bytes from them, stops
  gracefully on `SIGTERM`, and validates the unit (`systemd-analyze
  verify`, where systemd exists: CI) and the Caddyfile (`caddy
  validate`, in a pinned container). The workspace's static server now
  answers every non-file path under `/workspace` with the app shell, so
  a route that is also a directory of sub-routes (`/workspace/briefs`)
  loads instead of 404ing. Not tested, and said so in the guide: the
  worker's own container (D-062), and the notification webhook's TLS —
  the webhook adapter speaks plain HTTP until the e-mail adapter (I3).
  Cost: the deploy test adds about a minute to the gate.

## Decisions from the independent I1 white-hat review (2026-11-19)

- **D-065 — A session check is priced like the write it is.** The review
  showed `require_operator` ran its `UPDATE operator_sessions` on the
  shared pool with no budget at all: a flood of shaped-but-random cookies
  turned into unbounded writes against the same pool intake uses
  (invariant 5). Now a failed session check costs one of
  `SESSION_CHECK_FAILURES_PER_ADDRESS_PER_MINUTE` (20) tokens charged per
  client address after the trusted-proxy resolution; a live session (or a
  delete that found a row) refunds it, so the budget counts failures
  only. The checks themselves moved to their own four-connection pool
  with a 1 s acquire and statement timeout, like the prober's (D-047), so
  even a charged flood cannot starve intake's pool. `DELETE
  /v1/operator/session` is bounded the same way because it is a write
  too. Proven by the `SessionGate` unit tests and the flood drill
  (`operator::a_flood_of_bad_session_cookies_is_limited_and_leaves_intake_alone`,
  which returned 401 forever before).

- **D-066 — The worker confines itself; the deployment says so.** D-062
  owed the container "at deployment"; the review pointed out that same-
  user deployment still handed a decoder bug the server's environment
  (`/proc/<pid>/environ` holds the database URL and the operator hash),
  the root-readable env file, and loopback (the database, the webhook).
  Chosen instead of a container runtime: the worker installs its own
  seccomp filter before it reads a byte — a deny list answering EPERM
  that names every socket call, every file operation by name
  (`openat`/`openat2`/`open_by_handle_at`, the `*at` family and the
  legacy x86_64 calls), exec, new processes and namespaces, signals to
  other processes, mounts, ptrace and friends, inotify/fanotify, and
  io_uring — leaving it the stdin/stdout pipes it was started with and
  nothing else; the server goes non-dumpable (`PR_SET_DUMPABLE, 0`) so
  same-uid `/proc` reading and ptrace stop working; the env file becomes
  root-only `0600` (systemd reads it for the unit; the guide's `sudo -u
  intent sh -c '. env'` steps become `systemd-run -p EnvironmentFile=…
  -p User=intent`). `RestrictNamespaces` in the unit already keeps the
  child from making a user namespace to route around the filter. Proven
  by the `denied_syscalls` unit test and `the_process_goes_non_dumpable`
  on Linux. A missed syscall still runs — the deny list is not an
  allowlist — so rlimits, the empty environment and the one-job process
  model stay primary.

- **D-067 — The review's smaller hardening, batched.** (a) The SVG
  sanitizer no longer scans for literal `url(`: backslashes are refused
  outright (a CSS escape can spell any forbidden token), every `(`
  must open a name on a ten-function allowlist, `url()` must name a
  same-document fragment, and `<use>` expansion is preflighted for depth
  (8) and total cost (512) with cycle detection — a renderer's expansion
  is exponential otherwise. (b) `withdraw_fact` joins the other writes:
  `application/json` or the `x-intent` header is required, so a foreign
  form carrying the session cookie hears 415 before anything is
  withdrawn. (c) `hash-password` refuses under 12 characters (and over
  sign-in's own 1024-byte ceiling, so a hashed password can never be one
  sign-in refuses before comparing), and D-052's known-address rule gains
  its own budget: a known address spends a 100-per-day verification
  budget refunded on success, rather than skipping a bound entirely —
  and the content-type check runs before the per-address charge so junk
  cannot churn the bounded key table. (d) The Caddyfile sets
  `Strict-Transport-Security: max-age=63072000; includeSubDomains`. (e)
  The production profile refuses `INTENT_COOKIE_SECURE=false` and an
  `http://` webhook whose target is not loopback, `localhost` or a
  private address; TLS is not offered because the build's reqwest has
  none — a remote provider sits behind a local relay. (f) `check-config`
  no longer opens the store: it verifies the root exists-or-is-creatable
  and writable, the worker exists and the egress guard builds, without
  touching the live lock the old server holds through an upgrade. Each
  fix carries its test; the deploy drill now runs `check-config` against
  a running server and greps the Caddyfile for HSTS.

- **D-068 — Clients idle below the server's head-read bound.** hyper's
  `header_read_timeout` (`INTENT_HEADER_READ_TIMEOUT_MS`, 10 s) bounds
  every wait for a request head — including the idle wait on a kept-alive
  connection — and closes the connection without a response when it
  fires. A client or proxy pool holding a connection past the bound
  reuses a dead one and races the close (k6/Go: `readLoopPeekFailLocked`,
  status 0; a proxy: a 502 for a request the upstream never read). hyper
  offers no separate idle timer, so the contract lives client-side: the
  Caddyfile pins `keepalive 5s` in the upstream transport (the deploy
  test asserts it parses and stays below the configured bound), and the
  k6 scenarios set `noVUConnectionReuse` — the transport pools for 90 s
  with no idle knob, so each iteration opens a fresh connection and no
  reuse can outlive the bound. The connections drill pins the reuse
  contract the pools rely on: a request sent after the bound sees a
  whole response, or a clean close before its bytes — never a truncated
  response mid-request.

- **D-069 — Sign-out is synchronous: confirmed, or honestly failed.**
  `DELETE /v1/operator/session` has exactly two outcomes. The row's
  delete confirmed → 204, the confirmed digest earns a tombstone in
  memory for the session TTL, and repeat requests with the same cookie
  answer 204 without touching the database. The delete could not
  confirm (a lock, a pool error, a timeout) → 503 with the cookie
  KEPT — no tombstone, no claimed success — and the UI answers
  "Sign-out failed — retry" so the operator knows the session is still
  live. There is no background retry and no on-disk journal: the first
  implementation carried a retry task, a journaled pending set and a
  state directory, and each review round found it leaked a class of
  failure the synchronous answer never had (a tombstone for a delete
  never confirmed, a cancelled task leaving pending debt, a stale
  journal snapshot, a boot loop on an unwritable state dir). The
  dedicated one-connection `signout_db` pool stays — it exists so no
  unauthenticated flood can queue ahead of the delete, which is what
  keeps the honest 503 rare. A restart drops the tombstone memory and
  the row's `DELETE`/`SELECT` do the work again; nothing is owed across
  a boot. The same ruling binds the known-address seed: a restart marks
  an address only for the remainder of its OWN audit window (`at +
  30d`, never a fresh `now + 30d`), and only for sign-ins recorded
  under the CURRENT credential — each `operator.sign_in` audit row
  carries `credential_fp` (SHA-256 of the configured hash, truncated),
  so a rotation means a thief's last successful sign-in stops
  conferring the 100/day budget the moment it happens, at the cost of
  the operator's usual address also losing it until it signs in once
  under the new password — the trade-off a rotation is for.

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
- **Connections per IPv6 client (D-053).** Whether the connection count
  should be keyed by a /48 (or the default cap or the 64-per-address limit
  changed) so one IPv6 subscriber cannot fill the cap with 16 /64s; needs
  a measurement of real client spread before it is decided.
- **Deploy access per client (B-4).** The client's own Cloudflare account
  or the operator-account fallback (D-017).
