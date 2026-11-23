import { timingSafeEqual } from 'node:crypto'
import type { Pool } from 'pg'
import {
  redactTelegramTranscript,
  telegramConversationTurnSchema,
} from '../shared/telegram-conversations'
import { readBoundedBody } from './profile'

const reply = (body: unknown, status = 200) =>
  Response.json(body, { status, headers: { 'Cache-Control': 'no-store' } })
const retained = "received_at >= now() - interval '3 days'"

export function pruneTelegramConversations(pool: Pool) {
  return pool.query(
    "DELETE FROM telegram_conversation_turns WHERE received_at < now() - interval '3 days'",
  )
}

export async function ingestTelegramConversation(
  request: Request,
  pool: Pool,
  secret: string | undefined,
) {
  const supplied = request.headers.get('authorization') ?? ''
  const expected = `Bearer ${secret ?? ''}`
  if (
    !secret ||
    Buffer.byteLength(supplied) !== Buffer.byteLength(expected) ||
    !timingSafeEqual(Buffer.from(supplied), Buffer.from(expected))
  )
    return reply({ error: 'Unauthorized' }, 401)
  if (request.method !== 'POST') return reply({ error: 'Method not allowed' }, 405)
  if (
    request.headers.get('content-type')?.split(';')[0].trim().toLowerCase() !== 'application/json'
  )
    return reply({ error: 'Invalid request' }, 400)
  let input: unknown
  try {
    input = JSON.parse((await readBoundedBody(request, 65536)) ?? '')
  } catch {
    return reply({ error: 'Invalid request' }, 400)
  }
  const parsed = telegramConversationTurnSchema.safeParse(input)
  if (!parsed.success) return reply({ error: 'Invalid request' }, 400)
  const turn = {
    ...parsed.data,
    question: redactTelegramTranscript(parsed.data.question, 1000),
    reply: redactTelegramTranscript(parsed.data.reply, 3000),
  }
  const result = await pool.query(
    `INSERT INTO telegram_conversation_turns (trace, conversation_id, turn)
     VALUES ($1, $2, $3::jsonb) ON CONFLICT (trace) DO UPDATE SET trace = EXCLUDED.trace
     WHERE telegram_conversation_turns.conversation_id = EXCLUDED.conversation_id
       AND telegram_conversation_turns.turn = EXCLUDED.turn RETURNING sequence`,
    [turn.trace, turn.conversationId, JSON.stringify(turn)],
  )
  return result.rowCount ? reply({ ok: true }) : reply({ error: 'Conflict' }, 409)
}

export async function telegramConversations(request: Request, pool: Pool, before: string | null) {
  const url = new URL(request.url)
  if (request.method !== 'GET') return reply({ error: 'Not found' }, 404)
  if (url.pathname.endsWith('/conversations')) {
    const rows = await pool.query<{
      conversation_id: string
      last_seen: Date
      turn_count: number
      cursor: string
    }>(
      `SELECT conversation_id, max(received_at) last_seen, count(*)::int turn_count, max(sequence)::text cursor
       FROM telegram_conversation_turns WHERE ${retained} GROUP BY conversation_id
       HAVING ($1::bigint IS NULL OR max(sequence) < $1::bigint)
       ORDER BY max(sequence) DESC LIMIT 31`,
      [before],
    )
    return reply({
      items: rows.rows.slice(0, 30).map((row) => ({
        conversationId: row.conversation_id,
        lastSeenAt: row.last_seen.toISOString(),
        turnCount: row.turn_count,
      })),
      nextCursor: rows.rows.length > 30 ? rows.rows[29].cursor : null,
    })
  }
  const conversation = url.searchParams.get('conversation')
  if (!conversation || !/^[a-f0-9]{64}$/.test(conversation))
    return reply({ error: 'Invalid conversation' }, 400)
  const rows = await pool.query<{ turn: unknown; received_at: Date; cursor: string }>(
    `SELECT turn, received_at, sequence::text cursor FROM telegram_conversation_turns
     WHERE conversation_id = $1 AND ${retained} AND ($2::bigint IS NULL OR sequence < $2::bigint)
     ORDER BY sequence DESC LIMIT 51`,
    [conversation, before],
  )
  return reply({
    items: rows.rows.slice(0, 50).map((row) => {
      const { conversationId: _conversationId, ...turn } = telegramConversationTurnSchema.parse(
        row.turn,
      )
      return { ...turn, receivedAt: row.received_at.toISOString() }
    }),
    nextCursor: rows.rows.length > 50 ? rows.rows[49].cursor : null,
  })
}
