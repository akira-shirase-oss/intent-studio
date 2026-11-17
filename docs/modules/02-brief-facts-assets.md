# Module 02 — brief, facts, assets and references (I1)

What exists, how it behaves when things go wrong, and which test proves each claim. Decisions:
D-002, D-011–D-014, D-016, D-055–D-064.

## Records and rules

| Claim | Proven by |
|---|---|
| A fact is written once (kind, statement, source, an optional contact link) and approved separately: who, on which day, until when — at most 90 days, 30 for prices and opening hours | `intent-domain` `fact::tests` |
| A fact is current through its review-by day and stale the day after; a withdrawn fact is never usable | `fact::tests::a_fact_is_current_through_its_review_day_and_stale_after` |
| A brief's every field is bounded and every problem is named by path (`services[1].name: duplicate`) | `brief::tests::every_nested_problem_has_its_path` |
| A brief's claims must be approved, current facts of the same tenant when it is saved (and a composite foreign key keeps them in the tenant) | `brief::tests::claims_must_be_approved_current_facts_of_the_tenant`; drill `studio` |
| A stored brief is re-read through the same validation | `brief::tests::a_stored_brief_reads_back_through_the_same_validation` |
| The page tree (`quote-request@0`) refuses a claim slot whose fact is unknown, of another tenant, not in the brief, unapproved, stale, withdrawn or of the wrong kind | `page_tree::tests::a_claim_slot_needs_an_approved_current_fact_the_brief_lists` |
| The same tree validates on the review-by day and fails with `fact_stale` the day after (clock fixture) | `page_tree::tests::the_same_tree_goes_stale_when_the_clock_passes_review_by` |
| Links go only to an anchor, a page of the tree, a contact fact's link, or `https` on the brief's own domains | `page_tree::tests::links_go_only_where_the_allowlist_says` |
| Free text with an unbound claim ("licensed", "since 1985", "$99") fails with `unbound_claim` and the token; a bound fact that says it supports it | `claims::tests`, `page_tree::tests::invented_claims_in_free_text_are_unbound` |
| Unknown components and fields do not parse; the API reports their path | `page_tree::tests::unknown_components_and_fields_do_not_parse`; drill `studio` |

## The API

All operator routes, behind the session; `401` before a byte of the body is read; writes are
JSON only (uploads: multipart with `X-Intent-Upload`); every write commits with its audit row;
no route logs record content.

| Route | Does |
|---|---|
| `GET/POST /v1/operator/tenants` | Clients |
| `GET/POST /v1/operator/tenants/{t}/facts` | Facts, newest first, each with its status today |
| `POST …/facts/{f}/approval`, `…/withdrawal` | Record an approval (or re-confirm), withdraw |
| `GET/POST …/briefs`, `GET/PUT …/briefs/{b}` | Briefs; a change names its `expected_revision` (`409 revision_conflict` otherwise); `?revision=n` reads an older one |
| `POST …/briefs/{b}/spec-check` | Validates a page tree against the brief and the tenant's facts today; stores nothing |
| `GET/POST …/assets`, `GET …/assets/{a}/content` | Uploads through the worker; content served sandboxed |
| `GET/POST …/briefs/{b}/snapshots` | Fetch a reference through the egress guard; every outcome recorded |

Proven end to end by the drills `studio` (`briefs_and_facts_round_trip_and_refuse_by_name`) and
`assets` (`uploads_are_sanitized_or_refused_and_references_stay_public`), each ending in the
log-capture check over brief, fact, licence and lead content.

## Untrusted bytes

- **Uploads** run in `intent-worker`, one process per file, with an empty environment, its own
  CPU, file-size, descriptor and (Linux) memory limits, a wall-time kill and an output cap
  (D-062). The hostile SVG fixture keeps none of its 17 dangerous parts; DTDs, mismatched,
  truncated and oversized files are refused by name; images lose their metadata; a worker
  killed by a signal fails only its upload. `intent-worker` `tests/assets.rs`,
  `intent-render` driver tests, drill `assets`.
- **References** are fetched only by `intent-egress` (D-060): loopback, private, link-local,
  metadata, shared, reserved and documentation addresses (and their IPv6 and embedded forms)
  are refused before any connection; the DNS answer is pinned; every redirect is checked;
  `https` is never downgraded; 3 redirects, 10 s, 5 MiB. `intent-egress` `policy::tests` and
  `tests/guard.rs`; drill `assets` (a loopback listener sees zero connections).
- **The store** keeps bytes by SHA-256, atomically, verified on read (D-061). `intent-store`
  tests; the deploy test's backup and restore.

## The workspace

Facts, Briefs and Assets screens (D-063), proven by `apps/workspace/tests/studio.spec.ts` at
1440 and 360 px with axe in light and dark; screenshots in `docs/screens/`.

## Not built (and where it goes)

Storing page trees and rendering them (I2, and the component set is blocked on B-1); parsing a
reference's HTML (I2, in the worker); WOFF2 and WebP (I2); the worker's container (deployment,
D-062); idempotent creates (a create retried after a lost answer makes a second record the
operator can withdraw).
