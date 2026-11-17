# Load results

One JSON file per run of `scripts/load.sh`, named `<date>-<scenario>-<commit>.json`, kept
whether the run passed or not. Each records the machine, the commit, `uptime` before and
after, the configuration, every k6 metric, the reconciliation verdict (every acknowledged
submission checked against the database), the provider fixture's counts before and after
recovery, and how many server log lines held lead content (must be 0).

The unit is one lead submission to `intent-server` (D-025), with the notification provider
refusing every attempt for the whole run and 5% deliberate resubmissions (END_GOAL failure
case 3).

**These are development observations on a shared laptop, never production capacity.** The
load generator, the server and PostgreSQL (in Docker) share one machine, and other projects'
builds were running during every run listed below; the load average is in each file. A
latency threshold missed on a busy machine is kept as a miss, not rerun until green.

| Date | Scenario | Submissions | Rate | Commit | Load avg (1 min) before | Acknowledged / reconciled | p50 / p99 | Thresholds |
|---|---|---|---|---|---|---|---|---|
| 2026-11-17 | realism (50/s, 60 s) | 3,001 | 50.0/s | 50bc7d0 (tooling uncommitted, see file) | 27.42 | 100% / 0 violations, all outbox rows delivered after recovery | 24.9 ms / 402 ms | **missed** p99 < 100 ms; others held |
| 2026-11-17 | burst (250/s, 60 s) | 15,001 | 250.0/s | a5e0d6f | 44.87 | **invalid**: a stray server of mine shared the reused database and "delivered" outbox rows to its in-memory recorder; see the file's `contaminated` note | (6.1 ms / 64.8 ms, intake only) | not counted; rerun below |
| 2026-11-17 | burst (250/s, 60 s), rerun | 15,001 | 250.0/s | 5df19c5 | 17.37 | 100% / 0 violations; 14,251 rows delivered = 14,251 provider acceptances, 0 duplicates | 1.2 ms / 15.3 ms | held (p99 < 100 ms, ack rate > 99.9%, 0 dropped) |
| 2026-11-17 | notify-down (500/s, 100,000) | 99,230 (771 dropped) | 496.1/s | 5df19c5 | 16.07 (37.81 after; up to 78 during the drain) | 99.997% (3 honest 503s, 2 of them stored anyway) / 0 violations; 94,269 rows delivered = 94,269 provider acceptances, 0 duplicates, 0 uncertain | 4.7 ms / 109 ms | **missed** p99 < 100 ms and dropped < 10; ack rate held |
| 2026-11-17 | realism (50/s, 60 s), after the fix round | 3,001 | 50.0/s | bd8d4e6 | **4.73** (4.85 when k6 ended) | 100% / 0 violations; 2,851 rows delivered = 2,851 provider acceptances, 0 duplicates; logged at debug, 0 lines with lead content | 3.8 ms / 14.7 ms | held (p99 < 100 ms, ack rate > 99.9%, 0 dropped) |
| 2026-11-17 | burst (250/s, 60 s), after the fix round | 15,000 | 250.0/s | c405dce | **4.51** (14.75 when k6 ended: other builds started) | 100% / 0 violations; 14,250 rows delivered = 14,250 provider acceptances, 0 duplicates; logged at debug, 0 lines with lead content | 3.0 ms / 89.9 ms | held, narrowly (p99 < 100 ms; debug logging on, unlike the 15.3 ms run) |
| 2026-11-17 | notify-down (500/s, 100,000), after the fix rounds | 100,001 (0 dropped) | 500.0/s | e61d0c2 | 4.76 (6.46 after) | 100% / 0 violations; 95,001 leads stored once; 95,001 rows delivered = 95,001 provider acceptances, 0 duplicates, 0 dead | 1.6 ms / 5.4 ms | held (p99 < 100 ms, ack rate > 99.9%, 0 dropped) |
| 2026-11-17 | **full (500/s, 1,000,000)** | 1,000,001 (0 dropped) | 500.0/s | e61d0c2 | 3.17 (8.61 after: other work during the run) | 99.9999% (1 unanswered, not acknowledged) / 0 violations; 950,000 leads stored once; 594,803 delivered = 594,803 provider acceptances, 0 duplicates; 355,197 dead after 8 attempts (provider down the whole 33 min at the 1/60 retry scale) | 1.9 ms / 12.1 ms (p99.9 88 ms) | held (p99 < 100 ms, ack rate > 99.9%, 0 dropped) |

## Budgets

The intake budgets are D-054, set from the quiet runs above and enforced as k6 thresholds by
`scripts/load.sh`: realism p99 < 50 ms; burst p99 < 200 ms; `notify-down` and full p99 < 100 ms
and p99.9 < 250 ms; always an acknowledged rate above 99.9%, dropped iterations below 0.01%,
zero reconciliation violations and zero log lines with lead content. Runs made before D-054 were
judged against the then-proposed 100 ms p99 (their files say so); every quiet run meets its
D-054 budget.

## Owed

- Nothing for I0. The ship gate (I6) re-runs the full run on the release commit.
