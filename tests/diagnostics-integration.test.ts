import { expect, test } from 'bun:test'
import { Pool } from 'pg'
import { diagnosticsJourneysSchema, diagnosticsTimelineSchema } from '../shared/diagnostics'
import { pruneDiagnostics, pruneExpiredRecordings } from '../server/diagnostics'
import { createIdentity } from './integration-identity'
import { startIntegrationServer } from './integration-server'

const databaseUrl = process.env.TEST_RAILWAY_DATABASE_URL
const integration = databaseUrl ? test : test.skip

integration(
  'website diagnostics persist, deduplicate, authorize, paginate and expire',
  async () => {
    if (!databaseUrl) throw new Error('Disposable database is required')
    const pool = new Pool({ connectionString: databaseUrl })
    const identities: { userId: string; teacherId: string }[] = []
    let stop: (() => void) | undefined
    try {
      const { origin, child } = await startIntegrationServer(databaseUrl)
      stop = () => child.kill()
      const password = 'synthetic-diagnostics-password'
      async function account(role: 'administrator' | 'teacher', phone: string) {
        const name = `Synthetic diagnostics ${role}`
        const userId = await createIdentity(pool, name, password)
        const teacherId = crypto.randomUUID()
        identities.push({ userId, teacherId })
        await pool.query(
          "INSERT INTO teachers (id, auth_user_id, name, access_mode) VALUES ($1, $2, $3, 'read_write')",
          [teacherId, userId, name],
        )
        await pool.query(
          'INSERT INTO app_accounts (auth_user_id, role, teacher_id, verified_phone) VALUES ($1, $2, $3, $4)',
          [userId, role, teacherId, phone],
        )
        const login = await fetch(`${origin}/api/auth/sign-in/phone`, {
          method: 'POST',
          headers: { origin, 'content-type': 'application/json' },
          body: JSON.stringify({ phone, password }),
        })
        expect(login.status).toBe(200)
        const cookie = login.headers.getSetCookie()[0]?.split(';')[0]
        if (!cookie) throw new Error('Missing synthetic session')
        return { userId, headers: { cookie, 'x-user-id': userId } }
      }
      const teacher = await account('teacher', '+12025550181')
      const administrator = await account('administrator', '+12025550182')
      const journeyId = crypto.randomUUID()
      const entries = Array.from({ length: 51 }, (_, index) => ({
        id: crypto.randomUUID(),
        journeyId,
        step: index + 1,
        kind: 'screen',
        screen: 'attendance',
      }))
      const post = (body: unknown, headers = {}) =>
        fetch(`${origin}/api/diagnostics`, {
          method: 'POST',
          headers: { ...teacher.headers, origin, 'content-type': 'application/json', ...headers },
          body: JSON.stringify(body),
        })
      expect(
        (await post({ events: [entries[0]] }, { origin: 'https://foreign.example' })).status,
      ).toBe(403)
      expect(
        (await post({ events: [entries[0]] }, { 'x-user-id': administrator.userId })).status,
      ).toBe(401)
      expect((await post({ events: [{ ...entries[0], url: '/?q=private' }] })).status).toBe(400)
      expect((await post({ events: entries })).status).toBe(400)
      expect((await post({ events: [], oversized: 'x'.repeat(20_000) })).status).toBe(400)
      const firstPage = { events: entries.slice(0, 30) }
      const retries = await Promise.all([post(firstPage), post(firstPage)])
      expect(retries.map((response) => response.status)).toEqual([200, 200])
      expect((await post({ events: entries.slice(30) })).status).toBe(200)

      const listUrl = `${origin}/api/diagnostics/journeys?role=teacher&actor=${teacher.userId}`
      expect((await fetch(listUrl)).status).toBe(401)
      expect((await fetch(listUrl, { headers: teacher.headers })).status).toBe(403)
      expect(
        (await fetch(`${listUrl}&before=9999999999999999999`, { headers: administrator.headers }))
          .status,
      ).toBe(400)
      const list = await fetch(listUrl, { headers: administrator.headers })
      expect(list.status).toBe(200)
      expect(list.headers.get('cache-control')).toBe('no-store')
      expect(diagnosticsJourneysSchema.parse(await list.json()).items).toEqual([
        expect.objectContaining({
          journeyId,
          actorId: teacher.userId,
          actorName: 'Synthetic diagnostics teacher',
          role: 'teacher',
          eventCount: 51,
        }),
      ])
      const timelineUrl = `${origin}/api/diagnostics/events?actor=${teacher.userId}&journey=${journeyId}`
      const read = async (url: string) =>
        diagnosticsTimelineSchema.parse(
          await (await fetch(url, { headers: administrator.headers })).json(),
        )
      const timeline = await read(timelineUrl)
      expect(timeline.items).toHaveLength(50)
      expect(timeline.items[0]?.event.step).toBe(51)
      expect(timeline.nextCursor).toBeTruthy()
      const older = await read(`${timelineUrl}&before=${timeline.nextCursor}`)
      expect(older.items.map(({ event }) => event.step)).toEqual([1])
      expect(older.nextCursor).toBeNull()

      const retired = origin + '/api/diagnostics/replay'
      const legacyBatch = {
        method: 'POST',
        headers: { ...teacher.headers, origin, 'content-type': 'application/json' },
        body: JSON.stringify({
          journeyId,
          frames: [
            {
              kind: 'view',
              step: 1,
              timeMs: 100,
              screen: 'dashboard',
              width: 360,
              height: 800,
              theme: 'light',
              language: 'en',
              boxes: [],
            },
          ],
        }),
      }
      expect((await fetch(retired, legacyBatch)).status).toBe(400)
      expect((await fetch(retired, { headers: administrator.headers })).status).toBe(404)
      const recordings = () =>
        pool.query('SELECT step FROM website_session_replay WHERE actor_id = $1', [teacher.userId])
      expect((await recordings()).rows).toEqual([])
      await pool.query(
        `INSERT INTO website_session_replay (actor_id, journey_id, step, frame, received_at)
         VALUES ($1, $2, 1, '{}', now() - interval '4 days'), ($1, $2, 2, '{}', now())`,
        [teacher.userId, journeyId],
      )
      await pruneExpiredRecordings(pool)
      expect((await recordings()).rows).toEqual([{ step: 2 }])
      expect((await read(timelineUrl)).items).toHaveLength(50)

      await pool.query(
        'UPDATE app_accounts SET invitation_request_id = $2, invitation_requires_setup = true WHERE auth_user_id = $1',
        [teacher.userId, crypto.randomUUID()],
      )
      expect((await post({ events: [entries[0]] })).status).toBe(403)
      await pool.query('UPDATE app_accounts SET active = false WHERE auth_user_id = $1', [
        administrator.userId,
      ])
      expect((await fetch(listUrl, { headers: administrator.headers })).status).toBe(403)
      await pool.query('UPDATE app_accounts SET active = true WHERE auth_user_id = $1', [
        administrator.userId,
      ])
      await pool.query(
        "UPDATE website_diagnostics SET received_at = now() - interval '31 days' WHERE actor_id = $1",
        [teacher.userId],
      )
      expect((await read(timelineUrl)).items).toEqual([])
      await pruneDiagnostics(pool)
      expect(
        (
          await pool.query('SELECT 1 FROM website_diagnostics WHERE actor_id = $1', [
            teacher.userId,
          ])
        ).rowCount,
      ).toBe(0)
    } finally {
      stop?.()
      for (const { userId, teacherId } of identities) {
        await pool.query('DELETE FROM app_accounts WHERE auth_user_id = $1', [userId])
        await pool.query('DELETE FROM teachers WHERE id = $1', [teacherId])
        await pool.query('DELETE FROM neon_auth."user" WHERE id = $1', [userId])
      }
      await pool.end()
    }
  },
  20000,
)
