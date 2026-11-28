# Local performance lab

Run the lab from PowerShell with Node.js 24, npm 11, Docker Desktop and Playwright Chromium. It uses dedicated containers on `wartaq-perf-lab`, synthetic logins and a loopback-only web port. Each run clones one database template and drops its clone afterward.

Build the baseline image from the pinned source revision before measuring a candidate. This comparison used `7f5ae9a` and PostgreSQL migrations through 022 for its baseline.

```powershell
npm ci
npm exec -- playwright install chromium
./scripts/performance/prepare.ps1 -ThroughMigration 22
./scripts/performance/build.ps1 -Tag wartaq-perf:baseline
./scripts/performance/run.ps1 -Image wartaq-perf:baseline -Label baseline-a -Mode hplus
./scripts/performance/run.ps1 -Image wartaq-perf:baseline -Label baseline-b -Mode hplus
./scripts/performance/run.ps1 -Image wartaq-perf:baseline -Label baseline-load-a -Mode stress -Fixture large
./scripts/performance/run.ps1 -Image wartaq-perf:baseline -Label baseline-load-b -Mode stress -Fixture large
node scripts/performance/report.mjs .artifacts/performance/baseline-a-hplus.json
```

Run workloads serially and use fresh containers from the same two images in each final comparison. Both services have a one-CPU quota and a 512 MiB limit. The H+ model uses a 600 ms simulated latency, 400 kbit/s download, 100 kbit/s upload, and 2× CPU slowdown on emulated phones. It covers first sign-in, online teacher-to-student delivery, note save, warm reload, offline edits and reconnection. The LAN run also opens administrator Settings. The small fixture has 48 learners and 2,408 events. The stress fixture has 480 learners and 120,729 events. Seven concurrent authorized readers perform 56 validated reads. Multiple tabs can share the same signed-in account.

The proxy counts real HTTP messages, including service-worker fetches and socket upgrades. It counts encoded bodies and HTTP headers and includes WebSocket frames under their connection. It excludes TCP and TLS framing. Simulated network timing comes from Chromium. Container CPU uses the cgroup usage delta, and sampled working set uses current memory minus inactive file pages. Lifetime memory peaks are separate from the sampled working set. Compare equivalent successful jobs and report failed operations alongside byte counts.

Use `storage.sql` with `psql` against the appropriate local fixture. It reports logical row counts, relation bytes, and whole-database bytes. Measure `pg_wal` and the complete data directory separately when comparing Docker volume footprint. Equal rewrites on both databases avoid counting the compacting effect of index rebuilds as a schema saving.

For the compact schema comparison, prepare the baseline templates with `./scripts/performance/prepare.ps1 -ThroughMigration 22`, then run `./scripts/performance/compare-storage.ps1 -Label storage-a` and repeat with another label. The script compares identical database clones after equal event table rewrites and checks that the ordered canonical ledger digest matches. For candidate browser and load runs, pass `-Migrations @('neon/023-compact-attendance.sql','neon/024-ledger-invalidation.sql') -ReclaimEvents`. Use `-Mode hplus-dashboard -Fixture large` for administrator first open, attendance, summary, notes, correction and history paging. `-Mode hplus-warm` measures active work separately after a 95-second idle window. Set `WARTAQ_PERF_BLACKHOLE_WS=1` to test a network that silently drops WebSocket upgrades.

Use `-IsolateDatabase` to restore the same fixture into a fresh PostgreSQL container for each run when other scenarios might have changed the shared cluster's working set. `-Mode lan-preparation -Fixture large` checks that the large administrator notebook prepares a full offline copy on a fast link and opens its course summary after disconnecting.

For a runtime comparison, run `./scripts/performance/pair.ps1 -ControlImage wartaq-perf:candidate -CandidateImage wartaq-runtime:native-v1 -Label runtime-ab` after preparing the current migrations. The two web containers receive the same requests against separate copies of the small fixture. The report stores response digests and statuses. Add `-KeepRunning` to inspect the two loopback endpoints on ports 48717 and 48727. Stop `wartaq-pair-control` and `wartaq-pair-candidate` and drop `wartaq_pair_control` and `wartaq_pair_candidate` on the disposable `wartaq-perf-db` before re-creating a retained pair.

When Cargo registry downloads are unavailable in Docker, run `cargo fetch --locked --manifest-path rust-sync/Cargo.toml` on the host and use `build.ps1 -UseCachedRuntime`. It builds the frontend with the installed npm packages and builds Rust in an offline Docker container using the host Cargo registry and the same Bookworm compiler as the Dockerfile.

Reports and the generated fixture password live in the ignored `.artifacts/performance` directory. Stop only the named `wartaq-perf-db` container when finished. The lab has no provider connection.
