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
