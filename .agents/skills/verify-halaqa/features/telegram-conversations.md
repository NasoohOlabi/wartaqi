# Verify Telegram conversations

- Open Administrator Settings and Telegram using synthetic conversations. Check the list, empty and failed reads, pagination, and chronological messages, delivery and inference status, and timing. Verify messages render as text and no opaque identifiers are displayed.
- Check English and Arabic at desktop and 320px widths, including navigation back to the list and no horizontal overflow. Teachers and students must not see the view or issue diagnostic reads. `e2e/telegram-conversations.spec.ts` covers synthetic administrator and teacher paths.
- Verify the exact deployed commit and health on Railway. Live checks require synthetic data and disabled tracing; local browser tests do not prove provider behavior.
