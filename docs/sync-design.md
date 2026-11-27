# Workspace sync design and delivery plan

## User-visible contract

A previously prepared device opens the user's last authorized working data and lets staff take attendance and notes without waiting for Railway. Opening the site for the first time, signing in, switching identity, administering access and performing actions whose authority or external effects need immediate confirmation require a connection. Students can reopen only their own projected attendance. Never claim a durable save before IndexedDB commits. Distinguish saved on this device, accepted by Wartaq, waiting for review and rejected. Preserve the user's draft if fresh data arrives.

## Architecture decisions

1. Keep Railway Postgres, the existing role-checked projections, append-only event history, IndexedDB and the existing durable operation IDs. Integrate Socket.IO through its supported Bun transport. For a few concurrent devices use invalidation followed by a complete authorized projection. A sequence cursor over events alone misses changes to memberships, grants and courses. A replicated second database or CRDT would require another permission and conflict model.
2. Attach Postgres statement triggers to the six domain tables. Emit a content-free notification in the writing transaction. Each web process listens on a dedicated connection, reconnects with bounded backoff, and invalidates clients after it resumes listening. This covers web, CLI, and bot writes and avoids logging private data. Diagnostic telemetry must never invalidate the notebook that generated it.
3. Authorize the same-origin Socket.IO handshake using the existing HttpOnly session and application grant. Emit only a content-free workspace change hint. Every read and write still checks current access. Reconnect, foreground and periodic repair recover dropped hints. Scope subscriptions and asynchronous results to the active identity and dispose them when it changes.
4. Share one client change stream across mounted consumers. Coalesce bursts, allow one refresh in flight per projection and retain one trailing refresh if anything changed during the request. Publish accepted snapshots after validation. React observes a stable snapshot, so unrelated renders, filters, and route changes do not imply network activity. Preserve pending edits as an overlay over confirmed values.
   Course and grade write acknowledgements also request a snapshot that starts after the write. If a read was already running, the writer waits for it to finish before requesting the fresh snapshot.
5. Keep role-specific snapshot types. Staff need course, enrollment, roster, grade and history values that the legacy reduced snapshot omits. Students store only their own projected attendance. Credential management, invitation secrets and transcripts stay server-only. A known denial, role change, sign-out, switch or expired grant ends cached access. A new grant to the same identity must not resurrect data from a previous assignment.
6. Attendance compares the observed event ID, contacts compare the observed number, notes append with stable IDs. For other offline-editable fields, record the original field value and intended value. Server checks current authority, transactionally compares relevant fields, treats an already-achieved effect as success and returns a reviewable conflict on divergence. Never retry a non-idempotent administrative POST blindly. Preserve legacy pending batches, ordering and explicit retry rules. For changes requiring live validation expose the connection requirement before submission.
7. Keep conflict notices accessible, localized and specific. Let a user inspect saved and local values and decide which to keep. Silent reconciliation is appropriate for identical outcomes. A socket hint alone is insufficient evidence that the user's visible data changed.

## Work packages and integration order

| Package                  | Ownership and acceptance                                                                                                                                                                               |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Database and transport   | SQL migration, Bun listener, authenticated Socket.IO, transport tests. Demonstrate commit versus rollback, missed signals, restart and denied handshakes.                                              |
| Staff replica and queue  | Share the change-stream interface, hydrate validated local staff data promptly, preserve queued intent across refresh, close and reconnect. Show pending and conflicts through normal notebook routes. |
| Identity and role views  | Offline bootstrap, scoped student replica, administrator board and course views, grant expiry, switch and sign-out. Fresh pulls update visible data without losing form edits.                         |
| Domain writes            | Verify ordinary attendance, notes, roster and course/grade workflows individually. Persist supported commands locally, apply explicit field conflict policy and gate live-only actions.                |
| Integration and delivery | Independent production-browser and database tests, shell behavior, CI, release gate, exact Railway SHA, health and affected behavior. Record limitations and rollback procedure.                       |

## Acceptance

A warm visit with a stalled API displays cached records before the stalled response resolves. A prepared offline device can reload and perform its supported edits, close and reopen, then reconnect. Two independent sessions converge after one writes while the other stays open. Disconnecting mid-delivery or missing all socket messages converges on reconnect without duplicate events. Permission changes take precedence over cache and queue. Work on a shared browser never crosses accounts. New SQLite/Postgres replicas, infrastructure services and per-user socket payloads are unnecessary at this scale.

See [verification matrix](sync-verification.md) for release evidence.

## Adding a feature

1. Identify who can read the value, its authoritative entity ID, and the field or event version a user sees before editing. Define the Zod request and response in `shared/`. Keep authorization and commit order on the server.
2. Choose the existing staff notebook, administrator board or student projection. Add fields to that projection only for its authorized role. Subscribers register through `subscribeWorkspaceChanges` and release the subscription when they unmount. A view should keep its form state while the underlying projection refreshes.
3. For an editable value, save a stable operation ID, typed intent and observed value in the account's IndexedDB transaction. A device-level acknowledgment follows transaction completion. The projection overlays this pending intent until the server returns an outcome. Use the existing event ID for an append, and commit a receipt atomically with an update that might otherwise be repeated after an intervening edit.
4. Recheck the session, actor, grant and target row in the delivery transaction. Apply an independent field change, acknowledge an identical outcome, and return the current value when the same field diverged. User approval to keep their edit starts a new operation against the reviewed value. Preserve blocked work for inspection.
5. Changes to one of the six domain tables already wake subscribers after commit. If a new data source needs live display, first identify which clients need to re-read and whether the signal itself would disclose private activity. Exclude diagnostic collection from this path.
6. Add one real transaction test for the mutation and one browser test through its normal screen. Exercise a lost acknowledgment followed by another writer, plus the prepared-device case with the server unreachable. A new projection also needs a grant-change and cache-isolation check.

## Operational limits

The device that holds an unsent edit must run Wartaq again with connectivity for that edit to reach other users. Browser storage eviction or clearing site data can remove unsent work. A disconnected device can enforce locally known expiry but learns about an unexpected access withdrawal on reconnect. Data already viewed or downloaded cannot be recalled.

Full projections trade some network and database work for a simple recovery path. Three or four users can still accumulate a large history. Track compressed snapshot size, time to render a warm screen and revalidation latency as history grows. If a complete projection becomes too costly, narrow the projection or introduce a permission-aware cursor behind the same subscription interface.
