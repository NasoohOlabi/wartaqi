# Website diagnostics

Administrators can open **Settings → Diagnostics** to inspect recent signed-in website activity. A journey is one browser document under one active account. The list groups events by journey and shows the account, its role, the last receipt time, and the number of records. Each journey opens a timeline. A reload or a different tab starts another journey, and the account filter brings those journeys together.

## What a journey records

| Signal          | Recorded data                                 | How to interpret it                                                                                 |
| --------------- | --------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| Screen          | A fixed screen label                          | Navigation into a notebook, course, student portal, profile or settings screen                      |
| Action          | A known action and outcome                    | An activation, confirmed save, locally queued write, failed write or completed sync                 |
| Core Web Vitals | LCP and INP in milliseconds, and unitless CLS | Browser measurements for the document, displayed with the screen context available to the collector |
| Browser error   | Runtime or unhandled promise category         | A browser problem marker for the journey                                                            |

LCP measures loading, INP measures responsiveness, and CLS measures visual stability. Browser support and the way someone uses or closes the page affect which measurements arrive. An unavailable metric has no row. See [Core Web Vitals](https://web.dev/articles/vitals) and the [web-vitals library](https://github.com/GoogleChrome/web-vitals).

Event fields come from a fixed catalog. Screens use app enums and actions use stable labels at named controls and write interfaces. Payloads omit raw URLs, search strings, keystrokes, form values, note text, linked record identifiers, credentials, browser fingerprints, and error messages and stacks. The collector starts in the signed-in workspace. Public pages, unsuccessful sign-ins, Telegram and MCP are outside website journey coverage. These diagnostics describe behavior reported by a browser and serve operational investigation.

## Write and read interfaces

1. The browser collector accepts typed events and assigns the journey ID, sequence step and event ID. It batches the events in a bounded memory queue and uses the same account-bound JSON write transport as notebook requests. Navigation, user interactions, document metrics and write outcomes share this collector.
2. The same-origin Bun POST validates up to 30 events within 16 KiB and resolves the actor and role from the Better Auth session and current application access. The expected-account header prevents a queued batch from acquiring the next account's identity during a device account switch. Requests are limited to 20 accepted batches per account per minute per web instance. Exact replay of a request uses the same event IDs. Postgres assigns receipt time and stores diagnostic events in the dedicated `website_diagnostics` table.
3. The read interface checks current Administrator access separately. Parameterized, paginated queries provide the journey list and timeline. Account display names are resolved for display. Private responses use `Cache-Control: no-store`.

The analytics queue is best effort and isolated from notebook save and sync. A failed analytics send leaves the actual save result intact. The current document can retry after a network interruption. The browser can discard pending diagnostic records when a page closes, memory limits are reached, or the active account changes. Attendance and note durability still follow the notebook's existing write path. See [architecture](architecture.md).

## Operations and retention

- Apply `neon/015-website-diagnostics.sql` over the private Railway database connection before merging the web release. `/health` verifies the migration. Follow the ordered migration procedure in [Railway setup](railway-setup.md). The migration is additive, so a web rollback can leave the table in place.
- Events age out by server receipt time after 30 days. The diagnostics list and timeline honor that cutoff independently of the cleanup pass. Active web deployments remove expired rows periodically. Full infrastructure backups can contain diagnostic data and follow the provider backup lifecycle. The selective notebook transfer omits diagnostic history.
- Use synthetic accounts and records when verifying. Before merging, prove role and origin checks, rejection of unexpected fields, stable retry IDs, persistence across reload, account switch isolation, pagination, retention, and the English/Arabic layout at phone and desktop widths. After deployment, verify the exact `main` commit and health, then prove one synthetic journey from a browser through the Administrator timeline and remove its fixtures.

## Retired session recordings

The website no longer records or plays back sessions. Existing recordings expire through the original three-day cleanup. Full infrastructure backups follow the provider backup lifecycle.
