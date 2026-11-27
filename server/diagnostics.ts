import type { Pool } from 'pg'
import { z } from 'zod'
import {
  telegramConversationsPath,
  telegramConversationTurnsPath,
} from '../shared/telegram-conversations'
import { telegramConversations } from './telegramConversations'

const reply = (body: unknown, status = 200) =>
  Response.json(body, { status, headers: { 'Cache-Control': 'no-store' } })

export async function diagnostics(
  request: Request,
  pool: Pool,
  notebook: (request: Request) => Promise<Response>,
) {
  const url = new URL(request.url)
  const path = url.pathname
  // Older builds discard retired uploads on 400 without consulting the database.
  if (path === '/api/diagnostics' || path === '/api/diagnostics/replay')
    return reply(
      {
        error:
          path === '/api/diagnostics' ? 'Website diagnostics retired' : 'Session replay retired',
      },
      request.method === 'POST' ? 400 : 404,
    )
  if (path !== telegramConversationsPath && path !== telegramConversationTurnsPath)
    return reply({ error: 'Not found' }, 404)
  const actorId = request.headers.get('x-user-id')
  if (!actorId) return reply({ error: 'Unauthorized' }, 401)
  const me = await notebook(new Request(new URL('/api/me', url), { headers: request.headers }))
  if (!me.ok) return reply({ error: 'Forbidden' }, me.status)
  const account = z
    .object({
      account: z.object({
        role: z.literal('administrator'),
        invitationProfile: z.object({ requiresSetup: z.boolean() }).optional(),
      }),
    })
    .safeParse(await me.json())
  if (!account.success || account.data.account.invitationProfile?.requiresSetup)
    return reply({ error: 'Forbidden' }, 403)
  const before = url.searchParams.get('before')
  if (before !== null && (!/^[1-9]\d{0,18}$/.test(before) || BigInt(before) > 9223372036854775807n))
    return reply({ error: 'Invalid cursor' }, 400)
  return telegramConversations(request, pool, before)
}
