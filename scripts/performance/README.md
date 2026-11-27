# Local performance lab

Run the lab from PowerShell with Bun, Node, Docker Desktop and Playwright Chromium. It uses dedicated containers on `wartaq-perf-lab`, synthetic logins and a loopback-only web port. Each run clones one database template and drops its clone afterward.

```powershell
bun install --frozen-lockfile
bunx playwright install chromium
./scripts/performance/prepare.ps1
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

When registry downloads are unavailable in Docker, `build.ps1 -UseCachedRuntime` uses the existing `wartaqi:routed-web-current` dependency image and the same Bookworm Rust compiler as the production Dockerfile. It expects the cached Cargo registry archive under `.artifacts/performance`. The baseline lock comparison found identical package entries except for the removed `web-vitals` dependency. The candidate replaces the app source, built frontend and Rust executable in the cached image. The normal production Dockerfile is the portable path.

Reports and the generated fixture password live in the ignored `.artifacts/performance` directory. Stop only the named `wartaq-perf-db` container when finished. The lab has no provider connection.
