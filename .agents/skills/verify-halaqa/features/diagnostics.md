# Verify website telemetry retirement

- Open an old Diagnostics bookmark as an Administrator in English and Arabic at 320 and 1280 pixels. Settings opens on Accounts, with Telegram still available and no horizontal overflow. The browser sends zero website telemetry requests. The synthetic cases are in `e2e/diagnostics.spec.ts`.
- Save and clear attendance across two signed-in browsers, including offline reload and reconnect. Confirm delivery to the student and zero telemetry uploads in `e2e/workspace-sync.spec.ts`.
- Run the Rust and Bun parity test against migrated disposable Postgres. Confirm stale clients get HTTP 400 for retired uploads and HTTP 404 for retired reads.
- Verify the deployed commit and health in Railway. Apply migration 022 only after the new release passes health. Check table absence, compressed content-hashed assets and unversioned HTML and worker cache headers. See [Website telemetry retirement](../../../../docs/website-diagnostics.md).
