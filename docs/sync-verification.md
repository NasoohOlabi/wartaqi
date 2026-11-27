# Sync verification matrix

Use synthetic identities and independent browser contexts. Test through the same read, subscribe, queue and replay interfaces as production. Use a disposable migrated Postgres for transaction and session checks. A synthetic HTTP fixture is sufficient for deterministic browser timing and storage failures. Record which layer actually ran.

| Scenario                         | Observable result                                                                                                                                                                           |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Warm and cold launch             | A first visit requires authorized bootstrap. On a prepared device, with the next data GET deliberately held, an existing record is already visible.                                         |
| Shell restart                    | After one successful online visit, load a saved app URL with the browser offline. Its code, translations and role-specific data render.                                                     |
| Durable write                    | Save attendance and a note offline, close the page, reopen and inspect pending data. Release the network and observe one accepted effect per ID.                                            |
| Lost acknowledgement             | Commit a mutation, drop the response and replay it. Compare final server history, not only a toast or queue length.                                                                         |
| Collision                        | Two devices change the same effective mark to different values. Show the user's proposed value and server value. Choosing either survives reload. Identical edits converge silently.        |
| Concurrent refresh               | Block the first read, deliver multiple hints, finish it and assert exactly one trailing read. A disposed account cannot apply its late response.                                            |
| Two live devices                 | Authenticate independent Socket.IO clients, change a database record, assert the waiting UI updates without focus, manual reload or polling. Roll back a transaction and observe no change. |
| Listener and client interruption | Disconnect either side, commit multiple changes and recover. A complete authorized view includes the missed changes.                                                                        |
| Access boundary                  | Seed different teacher and student records. Inspect persisted student projection, account switch, explicit sign-out, confirmed denial, role downgrade and known expiry.                     |
| Storage failure                  | Reject a durable write. The UI retains the input and exposes failure. A failed cache update still allows a valid online read.                                                               |
| Layout and local speed           | Run Arabic and English, phone and desktop. Notices remain readable and operable. Navigate/filter cached screens with the data request stalled.                                              |
| Release                          | Run format, lint, typecheck, unit and browser suites, real database integration, artifact and dependency security checks. Confirm Railway health and deployment identity separately.        |

For rollback preserve existing receipt and ledger filters, IndexedDB pending IDs and all user work. Database notification triggers are additive. An old application simply ignores notifications. Avoid clearing client storage to resolve delivery problems.

## Reproducing the checks

Run `bun run verify:release` with the repository Bun version. To exercise real database delivery, point `TEST_RAILWAY_DATABASE_URL` at a **disposable** Postgres with the Railway and notebook migrations applied. CI creates fresh databases and performs these migrations before testing.

- `bun test tests/sync-transport.test.ts tests/grade-sync.test.ts` exercises actual socket transports, SQL transactions, listener recovery and grade receipts.
- `bun run build` followed by `bunx playwright test e2e/workspace-sync.spec.ts --project=chromium` starts the real Bun server, signs in separate synthetic users in isolated browser contexts, takes one offline, saves, reloads, reconnects, and checks the student UI and database count. It also holds a student GET open while verifying that saved data renders.
- `bun run verify:ui` exercises scoped storage, field conflicts, failure and retry, pending records, warm launches and English/Arabic layouts through normal screens. These synthetic HTTP cases complement the real server test. Physical browser storage eviction and installed-app behavior require separate device testing.
