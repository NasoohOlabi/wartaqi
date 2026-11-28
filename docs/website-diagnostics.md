# Website telemetry retirement

Wartaq no longer collects website journeys, click or write outcomes, Core Web Vitals, browser errors, or session recordings. Old Diagnostics bookmarks open Settings.

Both backend implementations return HTTP 400 for same-origin uploads from older tabs so those clients discard their queued telemetry. The obsolete read routes return 404. Rust serves those responses directly in production, keeping the compatibility process asleep. Telegram conversation troubleshooting remains available in Administrator Settings.

After the new web deployment is healthy, apply [migration 022](../neon/022-retire-website-telemetry.sql) to remove `website_diagnostics` and `website_session_replay` from live PostgreSQL. Historical migrations 015–017 stay in sequence for existing installations. Full infrastructure backups can contain the retired tables until those backups expire. After restoring an older full archive, apply migration 022 before opening it to traffic.

A rollback to a build that still requires the diagnostics table needs the empty schema from migrations 015–017 before that build can pass health checks. A forward fix preserves the retired state.
