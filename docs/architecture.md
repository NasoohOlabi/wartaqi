# Architecture and decisions

React uses same-origin Better Auth sessions and a Bun API backed by Railway Postgres. `app_accounts` maps authenticated identities to Administrator, Teacher or Student; `teachers`, `students`, `events`, `courses` and `course_enrollments` hold roster, ledger and course data. The internal `neon_auth` schema and some filenames retain their names for compatibility. See [roles](roles.md) and [Railway setup](railway-setup.md).

The server enforces the [role permissions](roles.md), grant expiry and current assignment on each request. Reassignment changes the staff scope of historical events while student access follows the linked student ID.

See [stale edits](stale-writes.md) for client edit timestamps, compatibility and offline write ordering.

## Events and projection

Attendance has three current statuses: **Present**, **Absent** and **Excused**. Tap the selected status again to clear the mark for that student, course and date. A clear appends an attendance event with a null status. The session becomes unmarked and is excluded from totals, grade attendance rates and the Student view. The clear retains its event ID for sync and can be reversed in history to reveal the previous surviving mark. Existing `early` and `late` values remain valid in raw events and pending retries for compatibility. Projection counts both as `present` and preserves the original payloads for exact duplicate-ID comparisons.

`shared/ledger.ts` defines validated event payloads: student snapshots, attendance, notes and reversals. Corrections append a reversal referencing a preceding non-reversal event; originals remain in history. Duplicate reversals are harmless; forward references and reversal-of-reversal are invalid. Projection uses database sequence order. The latest surviving attendance for a student/enrollment/date wins; historical unscoped events use a separate student/date namespace. Enrollment IDs are bound to their student by a database foreign key and validated against the active assigned roster before insertion; reversing it can reveal an earlier status. Notes remain separate. Duplicate event IDs count once, with conflicting values flagged. The API serializes validated payload JSON without a transport version field. Strict snapshot validation fails closed on malformed records; projection counts invalid correction references and conflicting IDs.

## Delivery

`src/useLedger.ts` persists a batch before sending and blocks new submissions while delivery is uncertain. Pending operations take a browser-wide Web Lock and the original account lock for compatibility with older open tabs. Retries and discards read the current stored batch again under those locks. Corrupt or unavailable storage and unsupported locks fail closed. A retry reads first and removes already visible IDs. The API enforces unique IDs so a retry cannot create another event for the same ID. The server checks duplicate IDs against the stored actor, assignment, PostgreSQL timestamp and payload. This legacy batch requires explicit retry. Pending storage is private data; reconcile it before clearing. Form drafts are memory-only. Active views also subscribe to workspace changes and revalidate on reconnect.

`bun run verify` checks the code and build; `bun run verify:ui` drives synthetic browser paths. Neither replaces live Railway verification. See the [verification skill](../.agents/skills/verify-halaqa/SKILL.md).

### Offline teacher notebook

A successful teacher notebook load stores the authorized active roster, phone numbers, course names and availability, the roster's memberships, and effective attendance and notes in account-scoped IndexedDB. The standard notebook renders these records and pending edits through its existing pages after a disconnect or offline restart. Teachers can select any date for attendance in an eligible course. Every assigned student can receive notes and edits to their name, phone, location and birth year, including students without a current enrollment. A fresh authorized snapshot replaces the saved roster and removes obsolete sessions without deleting queued work. Explicit sign-out, account switch, known grant expiry and confirmed access withdrawal close the offline notebook. The online sign-in cookie may expire while the device is offline. Browser storage still depends on the current browser profile and can be cleared by the device or browser.

The device confirms a save after the IndexedDB transaction commits. Each attendance tap and each note save take this path online as well as offline. Successive taps retain their order in the local queue. Sync shares the browser-wide write lock with the online ledger to protect uncertain deliveries. Foreground sync uses the same-origin session for the original account, then checks current permissions and assignments. `POST /api/attendance/sync` checks the effective mark version before appending. `POST /api/student/sync` appends notes and updates editable profile fields. Each changed field is compared with the value the editor originally saw under a row lock. The update and a private receipt commit together, so a retried operation cannot overwrite a later profile edit. The receipt contains a fingerprint rather than contact details and is excluded from notebook and student event projections. Divergent attendance and profile values are retained for review. A teacher can choose the server value or submit their value against the displayed version. Blocked writes remain visible and **Sync now** retries them after eligibility returns. Server order determines effective attendance, and accepted marks appear in the Student view.

### Workspace change stream

Successful notebook uploads release the delivery queue while the post-write snapshot refreshes. A slow read can delay readback, while the next locally saved edit can still reach the server. An authorization denial still triggers a checked refresh before the delivery queue proceeds.

Course edits accepted by the interface send in submission order. Readback and retry notices also follow submission order. A previous edit's snapshot refresh can finish while a later edit is already sending.

See the [sync design](sync-design.md) and [verification matrix](sync-verification.md). Each committed domain write triggers PostgreSQL NOTIFY. Each web process forwards a content-free Socket.IO hint. A single client connection wakes subscribed projections, which fetch through existing role-checked routes. A listener restart, client reconnect, foregrounding the page or a periodic visible-page check repairs missed hints. Administrator grades use field comparisons and operation receipts to reconcile local and server changes. Student devices persist only their own attendance projection. A first sign-in and authority-changing operations require a live server.

A server rollback after any phone edit must retain the receipt filter on all event projections. Older servers would otherwise reject the receipt as an unknown ledger event. Preserve receipt rows in backup and restore.

## Courses

Administrators manage courses and one enrollment per student/course. Scores are optional numbers from 0 to 100. The server locks the student during enrollment changes and attendance checks, validates scope and keeps enrollment IDs stable. Historical unscoped attendance remains unassigned. See [courses](courses.md).
