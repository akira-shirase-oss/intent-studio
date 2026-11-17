# Module 01 — the lead-intake spine (I0)

What exists, how it behaves when things fail, and which test proves each
claim. Decisions: D-005, D-019, D-020, D-029, D-030–D-037.

## The path

`POST /v1/sites/{site}/leads` (JSON, or a form post) →

1. **Shed** when 1,024 requests are already in flight: `503 overloaded`.
2. **Database down** (last probe failed): `503 unavailable`, `Retry-After: 2`.
3. **Abuse limits**: 5 a minute per client address (IPv6 by /64; the
   `X-Forwarded-For` hop is believed only from a configured proxy), 60 a
   minute per site, each in a bounded LRU table of 100,000 keys:
   `429 rate_limited` with `Retry-After`.
4. **Body**: 16 KiB cap (`413`), JSON or form only (`415`), unknown fields
   refused (`400 malformed_body`), honeypot (`400 rejected`), field rules
   (`422 validation_failed` with field codes, no values).
5. **Store** under a 3 s deadline: one statement inserts the lead and its
   outbox row (both or neither), unique on `(site, submission key)`.
   The key is the page's `submission_id`, or for a form without script a
   key derived from the site, the content and a 10-minute window (the
   previous window is checked too).
6. **Answer**: `201` with the lead id only after the commit returned;
   `200 duplicate: true` with the same lead for a resubmission of the same
   content; `409 submission_conflict` for the same id with other content;
   `404` for an unknown or closed site; any timeout or database error is a
   `503` that acknowledged nothing — resending with the same id is safe.

## The notifier

Claims due outbox rows under a 30 s lease (`FOR UPDATE SKIP LOCKED`), counts
the attempt at claim time, calls the provider with a 10 s deadline and no
transaction open, then records the D-030 decision while it still holds the
lease. It has its own connection pool and at most 32 attempts in flight, so
a refusing or hanging provider never takes intake's resources. A lease that
expires without a result makes the next attempt *uncertain* (counted on the
row): delivery is at least once. After 8 attempts, or a permanent provider
refusal, the row is `dead` and stays visible in the inbox; the lead is
untouched.

## Retention (not built yet)

Each lead stores its own `retain_until` (365 days by default, per tenant,
D-019/D-031). Nothing deletes leads yet: the purge of leads past
`retain_until` and deletion on request arrive at I3 (D-033). Until then a
lead stays until removed by hand.

## Lead states

`received` (memory only, never acknowledged) → `stored` (committed with its
outbox row: the acknowledgment point) → `notification_pending` →
`delivered` | `notification_dead`. `intent-domain::state` holds the
transition table and its tests.

## Proof

| Claim | Test |
|---|---|
| Field rules, keys, fingerprints, schedule, token buckets, states | `cargo test -p intent-domain` |
| One lead per retried submit, sequential and 20 at once; conflicts; derived keys | `intake::a_retried_submit_is_one_lead_sequentially_and_concurrently` |
| Oversized (declared and chunked), malformed, unknown-field, wrong type, honeypot, unknown and closed site, 405 envelope; nothing stored | `intake::bad_bodies_are_refused_with_named_codes_and_store_nothing` |
| Per-address and per-site limits; a forged header from an untrusted peer changes nothing | `intake::abuse_limits_hold_per_address_and_per_site` |
| Inbox needs a session; shows notification state; audit row; sign-out ends the session | `intake::the_operator_inbox_needs_a_session_and_shows_notification_state` |
| Provider refuses, then hangs: intake unaffected, nothing lost or duplicated, schedule floors hold, drains after recovery | `drill_notify_down::notify_down_loses_and_duplicates_nothing_and_never_blocks_intake` |
| Provider never recovers: dead after exactly 8 attempts, leads kept | `drill_notify_down::a_provider_that_never_recovers_ends_dead_and_visible_and_keeps_every_lead` |
| A lost answer after acceptance is retried and counted as uncertain | `drill_notify_down::a_lost_answer_after_acceptance_is_retried_and_counted_as_uncertain` |
| `kill -9` mid-request: every acknowledged lead durable, lost answers resend as the same lead, expired leases delivered | `drill_kill::kill_9_mid_request_loses_no_acknowledged_lead` |
| Postgres stopped mid-run: 503 + `Retry-After` only, readiness red then green, no loss after restart | `drill_db_down::db_down_refuses_honestly_and_recovers_without_loss` |
| No secret or lead content in any drill's log; the scanner catches a planted leak | `support::assert_no_leaks`, at the end of every drill |
| The inbox in Chromium at 1280 and 360 px: signed in and out with Tab and Enter only, focus visible; CSP with zero violations | `scripts/workspace-e2e.sh` |
| Slow headers, idle sockets and stalled readers are cut off; connections capped; a rate-limited body is never read (D-038) | `connections::*` (4 tests) |
| The site budget is spent only by storable leads for real sites; made-up sites create no rows (D-039) | `intake::only_storable_leads_from_known_sites_spend_the_site_budget` |
| Every `X-Forwarded-For` line counts, read from the right | `intake::every_forwarded_for_line_counts_so_a_client_cannot_pick_its_own_address` |
| A late result from an expired claim never overwrites the current one; rows nobody holds are swept (D-040) | `outbox_fencing::*` (2 tests) |
| The notifier uses only its own pool; readiness has its own connection (D-034, D-047) | `notifier_pool::the_notifier_never_touches_intakes_pool`, `readiness::readiness_stays_green_while_intakes_pool_is_saturated` |
| Sign-in: global budget, audit with actor/address/request id, `__Host-` cookie, idle timeout (D-041) | `operator::*` (2 tests) |
| Bidi controls and invisible characters refused; the consent notice version stored (D-042, D-044) | `lead::tests::bidi_controls_and_invisible_characters_are_refused`, `intake::the_consent_notice_version_is_required_and_stored_with_the_lead` |
| `/metrics` only for the host or a token holder (D-049) | `intake::metrics_are_for_the_host_or_a_token_holder_only` |
| The leak check proves every needle it is given | `log_scanner::*` |
| Load with the provider down, every acknowledgment reconciled | `scripts/load.sh`, results in `perf/results/` |
