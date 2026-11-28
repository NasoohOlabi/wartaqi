# Rust architecture audit and follow-up tasks

Reviewed on 2026-11-28. The highest-risk change is the attendance storage migration. The native reader reconstructs the original event, but an older web image reads the compact database value directly. With one synthetic teacher session, the published notebook returned HTTP 200 before migration 023 and HTTP 400 afterward. Its health endpoint returned 200 in both states. The native notebook returned 200 and the expected event after migration. Complete task 1 before enabling packing on a database shared with the older image.

## Which implementation owns a request?

| Boundary                 | Published source at `7f5ae9a`                                                                                                                                                                                                                      | Native rewrite at `394b13f`                                                                                                                                                                               |
| ------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Browser and offline data | React/Vite, same-origin sessions, IndexedDB projections and pending writes                                                                                                                                                                         | Same client boundaries, with additional partial notebook reads and cache handling                                                                                                                         |
| Web entry point          | Root Dockerfile starts Rust in hybrid mode. Rust serves assets, health, Socket.IO, retired telemetry paths and GETs for session, current account, notebook, admin board and student attendance. Other API requests go to the loopback Bun process. | Root Dockerfile ships the Rust executable and assets. Axum also owns auth mutations, catalog writes, OAuth, MCP and private Telegram endpoints.                                                           |
| Worker                   | Separate Bun Telegram service with encrypted SQLite state. The Rust worker crate is a library here.                                                                                                                                                | Separate Rust Telegram executable with the existing volume and polling protocol                                                                                                                           |
| Persistence and tooling  | Railway Postgres, ordered SQL migrations, Bun administration and selective transfer CLI                                                                                                                                                            | Same database and Auth schema. Migration 023 packs attendance and 024 distinguishes ledger notifications. Bun installs JS dependencies, Node runs the reference server, tests and administrative scripts. |

The native column refers to the committed local `local/wartaq-performance` branch, eight commits beyond the audited `main`. Read candidate-only paths with `git show 394b13f:path/to/file` in a clone containing the commit. Recheck against the intended release SHA before taking a task. The audit covers tracked architecture and test paths. Uncommitted work in the rewrite checkout, live account data, provider behavior and production performance were outside the evidence.

## Resolve before native cutover

- [x] **1. Make attendance rollback preserve new writes (native).** At `394b13f`, migration 023 packed stored JSON before the older reader was retired. The release now adds the compatible reader in 023 and enables packing in 025 after cutover. The Railway runbook documents unpacking before an image rollback. The prior hybrid image at `7f5ae9a` and the native image served identical teacher and student views before cutover and after rollback on a disposable PostgreSQL 17 clone. The comparison retained a mark inserted while packing was enabled. The SQL fixture checks ordered canonical payloads and exact retry comparisons, including irregular and legacy events. The versioned transfer test checks backup and restore.

- [x] **2. Restore the HTTP enrollment-save contract (native).** Axum now registers `POST /api/admin/enrollments/save` and uses the catalog handler already shared with MCP. The release-binary HTTP regression reproduced the 404 before the fix. It now verifies a saved explicit level, an explicit blank, preservation of the blank when the level is omitted, an unchanged roster level, and Teacher denial. The MCP integration covers the shared save operation.

- [ ] **3. Compare first writes on independent databases (both).** The `same` helper in `tests/rust-backend-parity.test.ts` sends the same request to both backends concurrently against one database. A successful first write on one side can turn the other side into a retry. Keep that shared-state test for interoperability. Reuse the candidate's `scripts/performance/pair.ps1` fixture-cloning approach for first-write comparisons.

  **Verify that** each runtime handles a fresh create, clear, correction, grade save and rejected batch independently, with matching event order, receipts, edit versions and rollback effects. Then replay across runtimes in both directions on shared state.

- [ ] **4. Put the web crate's own tests in required CI (both).** The image job builds the locked Rust web binary and exercises HTTP contracts. Building it omits the tests behind `#[cfg(test)]`. Neither audited workflow runs `cargo test` for `rust-sync`. The candidate runs the Telegram crate's tests. Building its `rust-common` dependency also leaves that crate's own bilingual search test unexecuted.

  **Verify that** required CI runs the locked web tests on a migrated disposable database, including the DB-gated cases, and reports their result separately from image/browser tests. Run the Telegram and shared crate tests where applicable. Cover both shipped Cargo lockfiles with dependency advisory review and updates. Dependabot currently tracks Bun and GitHub Actions, and the release advisory check invokes Bun.

- [ ] **5. Exercise the native Telegram worker and web server together (native).** The web integration uses TypeScript worker components, while Rust worker tests use test doubles for the application API. In `rust-telegram/src/polling.rs::poll_once` a valid update's offset is durable before the handler runs. A crash in between can skip the action. An undecodable final update can recur because its failure path leaves the offset unchanged.

  **Verify that** a fake Telegram provider drives the two real executables and disposable Postgres/SQLite through teacher linking, marking, student readback and revocation. Restart before the write and after commit but before reply. Specify how the user recognizes and recovers an uncertain action, demonstrate one ledger effect, and show the poller progresses past an unsupported update.

- [ ] **6. Verify large-notebook projections against the full ledger (native).** `api.rs::notebook_slice` can switch an administrator's focused reads to separate folds above 8,000 events. History also has a 100-row page boundary. The performance lab exercises large fixtures. The HTTP parity suite does not compare focused pages with the full projection at the transition.

  **Verify that** the same user sees identical effective marks, summary totals, notes and reversal state at 8,000 and 8,001 events for equivalent records. Put a correction and its target on different history pages. Reload offline after a focused read and verify previously prepared full data remains intact.

## Repair user-visible behavior on either runtime

- [ ] **7. Resolve a committed course edit after a lost acknowledgement.** `src/Courses.tsx::submit` retries an uncertain write with the original edit ID/time. The trigger in `neon/021-edit-versions.sql` permits an equal pair only within the original transaction. On disposable Postgres, a direct update committed once and the identical second transaction returned `W0001`. The course UI can therefore keep offering Retry for a change already saved.

  **Verify that** dropping the response after a membership or course-group change and retrying from the UI converges to its saved state. Preserve the newer value after a competing edit and reject reuse of the same identity for different input.

- [ ] **8. Distinguish session lookup outages from revoked access.** `rust-sync/src/api.rs::subject` discards an error from the session query. The request can return 401, which `src/NeonAuth.tsx` treats as a reason to clear prepared offline access. Native `mcp_http.rs::public` similarly maps a database failure during token validation to an invalid-token challenge.

  **Verify that** a transient lookup failure yields a retryable service error and the same user resumes after recovery with pending work intact. Keep expired, withdrawn and mismatched identities denied. Cover both browser and MCP responses without exposing credentials.

- [ ] **9. Scope pending local work to current access.** `src/offlineAttendanceStore.ts` keys attendance and student queues by user. Cached snapshots have a grant generation. `src/OfflineAttendance.tsx` can display saved conflict values for a student missing from the refreshed roster, and the sync loop selects pending work by user ID. Server authorization still applies to writes.

  **Verify that** a teacher with a reassigned student or a new grant cannot view out-of-scope conflict details or automatically submit old work. Preserve recoverable in-scope edits, including legacy queues. Check actual IndexedDB contents across reload and account switch. Prune superseded private snapshots separately from unsent work.

- [ ] **10. Make all accounts reachable from administration.** Both `staff_reads::admin_board` and the TypeScript board query use `ORDER BY u.id LIMIT 1000`. The browser searches only the returned accounts and receives no continuation.

  **Verify that** an administrator can locate and open the 1,001st account using deterministic paging or server-side lookup. Verify the browser and MCP account-directory consumers, including stable ordering and role restrictions.

## Operational follow-up

- [ ] **11. Bound repeat migration work (native).** The migration runner replays the ordered SQL. Files 001 and 021 recreate indexes that 023 drops, and 023 reinstalls attendance checks even after the initial backfill. A second run can scan a populated ledger while ending with the same schema.

  **Verify that** a fresh install, upgrade from 022 and second run produce the same constraints and canonical ledger digest. Measure locks and index builds on the second run with the existing large synthetic fixture. Run the standalone `tests/compact-attendance.sql` assertions alongside the transfer tests.

- [ ] **12. Carry the reviewed course state through a Telegram confirmation (native).** `rust-telegram/src/screens/courses.rs::commit` compares a saved confirmation with its latest read, then submits the payload in a separate MCP call. A browser write between those operations can be overwritten.

  **Verify that** a browser edit interleaved with a bot confirmation leaves the newer value intact and prompts a review. Verify a successful uncontested confirmation and an ambiguous response with the same operation ID.

Record completed tasks with the commit, runtime, fixture and observed outcome. Existing [sync verification](sync-verification.md), [stale-write semantics](stale-writes.md), [transfer and recovery](railway-setup.md#transfer-and-recovery) and the [migration plan](rust-backend-pilot.md) provide the domain contracts. Keep all rehearsal data synthetic.

## Reproduce the audit evidence

Use a disposable PostgreSQL 17 cluster and the source revision named in each task. Keep the published and candidate fixtures separate. The candidate's storage compatibility test also imports its TypeScript worker storage module through Node. Run it from a complete source snapshot with that revision's frozen dependencies installed.

| Check                                                                                                                                                                                                                                                | What it establishes                                                                                                 |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| Run `cargo test --locked --manifest-path rust-sync/Cargo.toml` with `TEST_RAILWAY_DATABASE_URL` pointed at the migrated fixture. Run the corresponding command for `rust-telegram/Cargo.toml`.                                                       | Executes crate tests. Some web tests return early without a database.                                               |
| On the candidate, run `psql -X -v ON_ERROR_STOP=1 -f tests/compact-attendance.sql` against an empty disposable database. Read the packed rows' stored and reconstructed JSON and validate each with the published `shared/ledger.ts` payload schema. | Distinguishes the candidate's successful round trip from the rollback reader's ability to consume the same data.    |
| With migration 021 applied, change one synthetic course in a transaction with an explicit edit timestamp and UUID. Commit, start a new transaction and repeat the identical update with that pair.                                                   | Isolates the database replay result in task 7. The lost browser acknowledgement remains an HTTP/UI acceptance test. |
