# Delivery status

Railway hosts the hybrid Rust/Bun web service, a separate Telegram worker and Postgres. The [Rust architecture audit](rust-architecture-audit.md) tracks follow-up work for this release and the full native rewrite. The prior Neon implementation is preserved at `backup/neon-current-2026-11-15` (`77cedae`); its data and services remain available for recovery. See [Railway setup](railway-setup.md) for cutover evidence, administration and recovery. Local and synthetic tests do not prove live provider behavior. Historical delivery records remain in [the work log](work-log.md).
