import type { Pool } from 'pg'
import { z } from 'zod'
import {
  diagnosticsBatchSchema,
  diagnosticsEventSchema,
  diagnosticsRoleSchema,
} from '../shared/diagnostics'
import { readBoundedBody } from './profile'
import { sessionReplay } from './sessionReplay'
import { replayPath } from '../shared/sessionReplay'

const reply = (body: unknown, status = 200) =>
  Response.json(body, { status, headers: { 'Cache-Control': 'no-store' } })
const actorSchema = z.string().min(1).max(256)
const expired = "received_at < now() - interval '30 days'"
const retained = `NOT (${expired})`
const writesPerMinute = new Map<string, number>()
let minute = -1

function acceptWrite(actor: string) {
  const current = Math.floor(Date.now() / 60000)
  if (current !== minute) {
    minute = current
    writesPerMinute.clear()
  }
  const writes = writesPerMinute.get(actor) ?? 0
  if (writes >= 20) return false
  writesPerMinute.set(actor, writes + 1)
  return true
}

export function pruneDiagnostics(pool: Pool) {
  return pool.query(`DELETE FROM website_diagnostics WHERE ${expired}`)
}

export async function diagnostics(
  request: Request,
  pool: Pool,
  notebook: (request: Request) => Promise<Response>,
) {
  const url = new URL(request.url)
  const actorId = request.headers.get('x-user-id')
  if (!actorId) return reply({ error: 'Unauthorized' }, 401)
  const me = await notebook(new Request(new URL('/api/me', url), { headers: request.headers }))
  if (!me.ok) return reply({ error: 'Forbidden' }, me.status)
  const account = z
    .object({
      account: z.object({
        role: diagnosticsRoleSchema,
        invitationProfile: z.object({ requiresSetup: z.boolean() }).optional(),
      }),
    })
    .safeParse(await me.json())
  if (!account.success || account.data.account.invitationProfile?.requiresSetup)
    return reply({ error: 'Forbidden' }, 403)
  const role = account.data.account.role
  if (url.pathname === replayPath) return sessionReplay(request, pool, actorId, role)
  if (url.pathname === '/api/diagnostics' && request.method === 'POST') {
    if (
      request.headers.get('content-type')?.split(';')[0].trim().toLowerCase() !== 'application/json'
    )
      return reply({ error: 'Invalid request' }, 400)
    let body: unknown
    try {
      body = JSON.parse((await readBoundedBody(request, 16384)) ?? '')
    } catch {
      return reply({ error: 'Invalid request' }, 400)
    }
    const batch = diagnosticsBatchSchema.safeParse(body)
    if (!batch.success) return reply({ error: 'Invalid request' }, 400)
    if (!acceptWrite(actorId)) return reply({ error: 'Too many diagnostic requests' }, 429)
    await pool.query(
      `INSERT INTO website_diagnostics (actor_id, event_id, journey_id, role, event)
      SELECT $1, (e->>'id')::uuid, (e->>'journeyId')::uuid, $2, e FROM jsonb_array_elements($3::jsonb) e
      ON CONFLICT (actor_id, event_id) DO NOTHING`,
      [actorId, role, JSON.stringify(batch.data.events)],
    )
    return reply({ ok: true })
  }
  if (role !== 'administrator') return reply({ error: 'Forbidden' }, 403)
  const before = url.searchParams.get('before')
  if (before !== null && (!/^[1-9]\d{0,18}$/.test(before) || BigInt(before) > 9223372036854775807n))
    return reply({ error: 'Invalid cursor' }, 400)
  if (url.pathname === '/api/diagnostics/journeys' && request.method === 'GET') {
    const actor = url.searchParams.get('actor')
    const filterRole = url.searchParams.get('role')
    if (actor !== null && !actorSchema.safeParse(actor).success)
      return reply({ error: 'Invalid account' }, 400)
    if (filterRole !== null && !diagnosticsRoleSchema.safeParse(filterRole).success)
      return reply({ error: 'Invalid role' }, 400)
    const rows = await pool.query<{
      actor_id: string
      journey_id: string
      role: string
      last_seen: Date
      event_count: number
      cursor: string
      name: string | null
    }>(
      `WITH recent AS (
        SELECT d.actor_id, d.journey_id, (array_agg(d.role ORDER BY d.sequence DESC))[1] role,
          max(d.sequence) cursor, max(d.received_at) last_seen, count(*)::int event_count
        FROM website_diagnostics d
        WHERE ${retained} AND ($1::text IS NULL OR d.actor_id = $1)
        GROUP BY d.actor_id, d.journey_id
        HAVING ($3::bigint IS NULL OR max(d.sequence) < $3::bigint)
           AND ($2::text IS NULL OR (array_agg(d.role ORDER BY d.sequence DESC))[1] = $2)
        ORDER BY max(d.sequence) DESC LIMIT 31
      )
      SELECT recent.*, u.name FROM recent LEFT JOIN neon_auth."user" u ON u.id::text = recent.actor_id
      ORDER BY recent.cursor DESC`,
      [actor, filterRole, before],
    )
    const items = rows.rows.slice(0, 30).map((r) => ({
      journeyId: r.journey_id,
      actorId: r.actor_id,
      actorName: r.name,
      role: diagnosticsRoleSchema.parse(r.role),
      lastSeenAt: r.last_seen.toISOString(),
      eventCount: r.event_count,
    }))
    return reply({
      items,
      nextCursor: rows.rows.length > 30 ? rows.rows[29].cursor : null,
    })
  }
  if (url.pathname === '/api/diagnostics/events' && request.method === 'GET') {
    const journey = z.uuid().safeParse(url.searchParams.get('journey'))
    const actor = actorSchema.safeParse(url.searchParams.get('actor'))
    if (!journey.success || !actor.success) return reply({ error: 'Invalid journey' }, 400)
    const rows = await pool.query<{ event: unknown; received_at: Date; cursor: string }>(
      `SELECT event, received_at, sequence::text cursor FROM website_diagnostics WHERE actor_id = $1 AND journey_id = $2 AND ${retained} AND ($3::bigint IS NULL OR sequence < $3::bigint) ORDER BY sequence DESC LIMIT 51`,
      [actor.data, journey.data, before],
    )
    return reply({
      items: rows.rows.slice(0, 50).map((r) => ({
        event: diagnosticsEventSchema.parse(r.event),
        receivedAt: r.received_at.toISOString(),
      })),
      nextCursor: rows.rows.length > 50 ? rows.rows[49].cursor : null,
    })
  }
  return reply({ error: 'Not found' }, 404)
}
