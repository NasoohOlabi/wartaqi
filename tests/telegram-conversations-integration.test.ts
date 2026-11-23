import { expect, test } from 'bun:test'
import { Pool } from 'pg'
import { diagnostics } from '../server/diagnostics'
import {
  ingestTelegramConversation,
  pruneTelegramConversations,
} from '../server/telegramConversations'
import { telegramConversationTurnsSchema } from '../shared/telegram-conversations'

const databaseUrl = process.env.TEST_RAILWAY_DATABASE_URL
const integration = databaseUrl ? test : test.skip

integration('Telegram transcripts authenticate, deduplicate, paginate and expire', async () => {
  if (!databaseUrl) throw new Error('Disposable database is required')
  const pool = new Pool({ connectionString: databaseUrl })
  const conversationId = crypto.randomUUID().replaceAll('-', '').repeat(2)
  const tracePrefix = crypto.randomUUID().replaceAll('-', '').slice(0, 8)
  const secret = 'synthetic-worker-secret'
  const turn = {
    trace: `${tracePrefix}0000`,
    conversationId,
    question: 'Question',
    reply: 'Answer',
    delivery: 'sent',
    outcome: 'ok',
    promptVersion: 'wartaq-help-v1',
    model: 'synthetic',
    elapsedMs: 1,
  }
  const post = (body: unknown, token = secret) =>
    ingestTelegramConversation(
      new Request('http://localhost/api/telegram/conversations', {
        method: 'POST',
        headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
        body: JSON.stringify(body),
      }),
      pool,
      secret,
    )
  const read = (role: string, query = '') =>
    diagnostics(
      new Request(
        `http://localhost/api/diagnostics/telegram/turns?conversation=${conversationId}${query}`,
        { headers: { 'x-user-id': 'synthetic' } },
      ),
      pool,
      async () => Response.json({ account: { role } }),
    )
  try {
    expect((await post(turn, 'wrong')).status).toBe(401)
    expect((await post({ ...turn, extra: 'invalid' })).status).toBe(400)
    expect((await post(turn)).status).toBe(200)
    expect((await post(turn)).status).toBe(200)
    expect((await post({ ...turn, reply: 'Different' })).status).toBe(409)
    expect((await read('teacher')).status).toBe(403)
    for (let i = 1; i <= 50; i++) {
      expect(
        (await post({ ...turn, trace: `${tracePrefix}${i.toString().padStart(4, '0')}` })).status,
      ).toBe(200)
    }
    const first = telegramConversationTurnsSchema.parse(await (await read('administrator')).json())
    expect(first.items).toHaveLength(50)
    expect(first.nextCursor).not.toBeNull()
    const second = telegramConversationTurnsSchema.parse(
      await (await read('administrator', `&before=${first.nextCursor}`)).json(),
    )
    expect(second.items).toHaveLength(1)
    await pool.query(
      "UPDATE telegram_conversation_turns SET received_at = now() - interval '4 days' WHERE conversation_id = $1",
      [conversationId],
    )
    expect(
      telegramConversationTurnsSchema.parse(await (await read('administrator')).json()).items,
    ).toHaveLength(0)
    await pruneTelegramConversations(pool)
    expect(
      (
        await pool.query('SELECT 1 FROM telegram_conversation_turns WHERE conversation_id = $1', [
          conversationId,
        ])
      ).rowCount,
    ).toBe(0)
  } finally {
    await pool.query('DELETE FROM telegram_conversation_turns WHERE conversation_id = $1', [
      conversationId,
    ])
    await pool.end()
  }
})
