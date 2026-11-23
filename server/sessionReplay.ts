import type { Pool } from 'pg'
import { z } from 'zod'
import type { DiagnosticsRole } from '../shared/diagnostics'
import {
  replayBatchSchema,
  replayFrameNumberSchema,
  replayFrameSchema,
  replayMaxBodyBytes,
} from '../shared/sessionReplay'
import { readBoundedBody } from './profile'

const reply = (body: unknown, status = 200) =>
  Response.json(body, { status, headers: { 'Cache-Control': 'no-store' } })
const writes = new Map<string, number>()
let minute = -1
const expired = "received_at < now() - interval '3 days'"

export function pruneSessionReplay(pool: Pool) {
  return pool.query(`DELETE FROM website_session_replay WHERE ${expired}`)
}

export async function sessionReplay(
  request: Request,
  pool: Pool,
  actorId: string,
  role: DiagnosticsRole,
) {
  const url = new URL(request.url)
  if (request.method === 'POST') {
    if (
      request.headers.get('content-type')?.split(';')[0].trim().toLowerCase() !== 'application/json'
    )
      return reply({ error: 'Invalid request' }, 400)
    let body: unknown
    try {
      body = JSON.parse((await readBoundedBody(request, replayMaxBodyBytes)) ?? '')
    } catch {
      return reply({ error: 'Invalid request' }, 400)
    }
    const batch = replayBatchSchema.safeParse(body)
    if (!batch.success) return reply({ error: 'Invalid request' }, 400)
    const current = Math.floor(Date.now() / 60000)
    if (current !== minute) {
      minute = current
      writes.clear()
    }
    const count = writes.get(actorId) ?? 0
    if (count >= 60) return reply({ error: 'Too many replay requests' }, 429)
    writes.set(actorId, count + 1)
    await pool.query(
      `INSERT INTO website_session_replay (actor_id, journey_id, step, frame)
      SELECT $1, $2::uuid, (f->>'step')::int, f FROM jsonb_array_elements($3::jsonb) f
      ON CONFLICT (actor_id, journey_id, step) DO NOTHING`,
      [actorId, batch.data.journeyId, JSON.stringify(batch.data.frames)],
    )
    return reply({ ok: true })
  }
  if (request.method !== 'GET') return reply({ error: 'Not found' }, 404)
  if (role !== 'administrator') return reply({ error: 'Forbidden' }, 403)
  const keys = [...url.searchParams.keys()]
  if (
    keys.some((key) => !['actor', 'journey', 'after'].includes(key)) ||
    new Set(keys).size !== keys.length
  )
    return reply({ error: 'Invalid query' }, 400)
  const actor = z.string().min(1).max(256).safeParse(url.searchParams.get('actor'))
  const journey = z.uuid().safeParse(url.searchParams.get('journey'))
  const afterText = url.searchParams.get('after')
  const after =
    afterText === null
      ? null
      : /^[1-9]\d{0,2}$/.test(afterText)
        ? replayFrameNumberSchema.safeParse(Number(afterText))
        : null
  if (!actor.success || !journey.success || (afterText !== null && !after?.success))
    return reply({ error: 'Invalid query' }, 400)
  const rows = await pool.query<{ frame: unknown }>(
    `SELECT frame FROM website_session_replay WHERE actor_id = $1 AND journey_id = $2 AND step > $3 AND NOT (${expired}) ORDER BY step LIMIT 21`,
    [actor.data, journey.data, after?.success ? after.data : 0],
  )
  const items = rows.rows.slice(0, 20).map((row) => replayFrameSchema.parse(row.frame))
  return reply({ items, nextAfter: rows.rows.length > 20 ? items[items.length - 1].step : null })
}
