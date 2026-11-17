# Workspace screens

Every workspace screen at 360 px and 1440 px, in light and dark, as the Playwright journeys left
it (`apps/workspace/tests/`). For review: the journeys' assertions and axe checks are what pass or
fail (D-063). Regenerate with `scripts/screens.sh` (needs the local stack) and commit when the
screens change.

| Screen | State shown |
|---|---|
| `sign-in` | Signed out |
| `inbox` | Three leads, notified |
| `facts-empty` | A new client with no facts |
| `facts-approval-refused` | An approval the server refused, explained at the field |
| `facts` | Approved, ran-out and withdrawn facts, after a withdrawal |
| `briefs-empty` | No brief yet |
| `brief-form-problems` | A brief saved empty: every missing part named and linked |
| `brief-view` | A saved brief, its reference refused by the egress guard and recorded |
| `briefs` | One brief at revision 2 |
| `assets-empty` | No files yet |
| `assets` | The hostile SVG saved with 17 parts removed; a broken file refused |

Bundle (gzip, `scripts/check-bundle.sh`): all JavaScript 115 KiB of a 160 KiB budget, largest
chunk 32 KiB of 48, CSS 32 KiB of 48 — measured 2026-11-17.
