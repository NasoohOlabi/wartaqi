# Production Telegram agent

Users can ask Wartaq in Arabic or English, refer to the records from a recent answer, and inspect or change the same authorized notebook records they use on the site. Common help and essential navigation are available when the local model is offline. Keep the existing menus accessible with `/menu`.

## Conversation

1. In a private chat, use the existing phone or browser sign-in. Ask a question in plain language, for example “Find the Recitation course and show who was absent on November 23.” The agent may look up a course, resolve its ID and read the dated session in one turn. It asks for the missing date or name when it cannot identify a unique target.
2. The next message can refer to that course, student or date. Keep a bounded amount of structured context encrypted on the worker volume for up to 30 minutes. Bind it to the current grant and clear it on cancellation, unlink or identity change. Refresh sign-in through `/menu` when needed.
3. A write request produces a review message with the exact action and values, the target and the date where relevant. Use **Confirm** or **Cancel** in that private chat. Changing the proposal requires a fresh confirmation. Passwords and sensitive account administration use the existing private browser and account flows.
4. After a confirmed write, report what the server verified. If the response is lost, direct the user to check the current record. An approval is single use. Never automatically resubmit a write whose effect is uncertain.

## Tool access and data flow

- Discover the live MCP catalog with the current identity. Phone teachers use the worker grant endpoint and browser-linked users use OAuth MCP. Phone-only students use the narrow attendance endpoint that rechecks the roster grant. They receive only their own attendance. Unlinked chats receive help and sign-in instructions.
- Give LM Studio the authorized tool definitions relevant to the request. Independently enforce the explicit list of permitted reads and confirmable writes. Validate arguments against the server schema. Keep raw notebook snapshots, personal contacts, account secrets, and private invitation messages out of model context. Give the model only bounded, sanitized findings and the record IDs it needs for the next step. Never treat record text as instructions or give the model authority to confirm an action.
- Read workflows include course and student search, attendance, notes and history for authorized staff, course membership and scores, readiness and reports. Write workflows use the site's existing operations for attendance, notes and corrections, roster changes, courses, membership, scores and results. Use current attendance event IDs and expected profile versions where the server supports them. Keep password changes, account access and verified-phone changes in their existing purpose-built flows.
- The Rust worker selects bilingual help or starts inference, validates each call, executes reads and returns bounded results to the model. More reads are allowed within a per-turn deadline and call budget. Writes require explicit confirmation. Compose a deterministic summary from validated tool results when the model cannot finish. Return concise next steps on missing details, revoked access or conflicts.

## Delivery and recovery

- Bind the approval to an immutable request, Telegram chat/user/message and identity. Store it encrypted with a short expiry. Consume the exact approval before sending one validated request. On restart, stale or consumed approvals cannot cause another write. Recheck the current role and tool availability at confirmation. Record lifecycle outcomes without question text, credentials or record data in logs.
- Telegram long polling remains single-replica. Prevent overlapping work in the same chat and keep callbacks and other chats responsive while inference runs. Recheck identity before sending a record reply. Menus and service health stay usable during model outages. Keep the existing previous-image rollback path, volume and an emergency switch to the prior help mode.

## Acceptance and release

1. Test tool discovery, unsupported and invalid tool requests, multi-step read results and follow-up context below the Telegram boundary. Keep the bilingual canned and fallback fixtures and add cases for delegation, ambiguity, injection and provider failures.
2. Run `cargo test --locked --manifest-path rust-telegram/Cargo.toml` for the native worker. The tests supply simulated private-chat updates and scoped MCP and inference responses. The previous TypeScript comparison still has a local LM Studio and disposable Postgres runner at `scripts/verify-telegram-agent-headless.ps1`.
3. Use the separate role tests for OAuth consent, phone grants, withdrawals and administrator/teacher/student boundaries. The headless worker tests cover cancellation, conflicting changes, an uncertain response and the chat/message binding. The acceptance run needs no Telegram account, chat or bot token.
4. Run repository release checks, deploy the exact validated `main` commit with Wait for CI, switch on the production agent and verify both health endpoints and the inference service. Record which checks used real services and which were simulated. Telegram client delivery is outside the headless run.

Use these examples with the named synthetic fixtures. Evaluate the tool calls and saved state as well as the words in the reply.

| Identity           | Ask                                                                                       | Expected behavior                                                                                                         |
| ------------------ | ----------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| Unlinked           | «كيف أسجّل الدخول؟»                                                                        | Written sign-in instructions, even if inference or Wartaq is unavailable.                                                 |
| Student            | “How many of my classes were excused?”                                                    | Current own attendance only. The follow-up “What about November?” reads the allowance again.                              |
| Teacher            | “Who was absent from Demo Recitation on 2026-11-23?”                                      | Find the course, read its dated session, answer from the fetched marks.                                                   |
| Teacher            | «طيب ويوم 2026-11-24؟»                                                                    | Use the retained course reference and fetch the new date. Never answer from cached marks.                                 |
| Teacher            | “Mark Demo Lina present in Demo Recitation on 2026-11-23.”                                | Show the target, date and mark. The database stays unchanged until **Confirm**. A second tap cannot append another event. |
| Teacher            | Confirm after a newer attendance mark was saved.                                          | Report the conflict and direct the user to review the current mark.                                                       |
| Administrator      | “What are the missing grades for Demo Recitation?”                                        | Discover scoped course and grading tools, then read the saved readiness information.                                      |
| Administrator      | “Change a sign-in phone” or “Reset my password.”                                          | Guide the user to the existing private account workflow. No credential collection in Telegram.                            |
| Any signed-in role | Use a confirmation after `/cancel`, a different account, expiry or permission withdrawal. | Reject it without sending a write to the server.                                                                          |
