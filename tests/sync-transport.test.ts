import { expect, test } from 'bun:test'
import { Pool } from 'pg'
import { io, type Socket } from 'socket.io-client'
import { z } from 'zod'
import { createIdentity } from './integration-identity'
import { startIntegrationServer } from './integration-server'

// Only use a disposable, migrated database.
const databaseUrl = process.env.TEST_RAILWAY_DATABASE_URL
const integration = databaseUrl ? test : test.skip

async function connect(socket: Socket) {
  const result = Promise.withResolvers<string>()
  socket.once('connect', () => result.resolve('connected'))
  socket.once('connect_error', (error) => result.resolve(error.message))
  socket.connect()
  return result.promise
}

integration(
  'authorized real Bun sockets receive committed changes and recover a terminated listener',
  async () => {
    if (!databaseUrl) throw new Error('Disposable database required')
    const target = new URL(databaseUrl)
    target.searchParams.set('application_name', `sync-test-${crypto.randomUUID()}`)
    const pool = new Pool({ connectionString: databaseUrl })
    const teacherId = crypto.randomUUID()
    const password = 'synthetic-test-password'
    const phone = '+12025550419'
    const sockets: Socket[] = []
    let server: Awaited<ReturnType<typeof startIntegrationServer>> | undefined
    let userId: string | undefined
    try {
      userId = await createIdentity(pool, 'Synthetic Sync Teacher', password)
      await pool.query(
        "INSERT INTO teachers (id, auth_user_id, name, access_mode) VALUES ($1, $2, 'Sync Teacher', 'read_write')",
        [teacherId, userId],
      )
      await pool.query(
        "INSERT INTO app_accounts (auth_user_id, role, teacher_id, verified_phone) VALUES ($1, 'teacher', $2, $3)",
        [userId, teacherId, phone],
      )
      const triggerTables = await pool.query(
        `SELECT event_object_table FROM information_schema.triggers
         WHERE trigger_schema = 'public' AND trigger_name = 'notify_workspace_changed'
         GROUP BY event_object_table ORDER BY event_object_table`,
      )
      expect(triggerTables.rows.map((row) => row.event_object_table)).toEqual([
        'app_accounts',
        'course_enrollments',
        'courses',
        'events',
        'students',
        'teachers',
      ])
      server = await startIntegrationServer(target.href)
      const { origin } = server
      async function listenerId() {
        const result = await pool.query(
          `SELECT pid FROM pg_stat_activity WHERE application_name = $1
           AND query = 'LISTEN wartaq_workspace_changed' AND state = 'idle'`,
          [target.searchParams.get('application_name')],
        )
        return z.number().optional().parse(result.rows[0]?.pid)
      }
      let firstPid = await listenerId()
      for (let attempt = 0; firstPid === undefined && attempt < 20; attempt++) {
        await Bun.sleep(25)
        firstPid = await listenerId()
      }
      expect(firstPid).toBeDefined()
      async function login() {
        const response = await fetch(`${origin}/api/auth/sign-in/phone`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', origin },
          body: JSON.stringify({ phone, password }),
        })
        expect(response.status).toBe(200)
        return response.headers
          .getSetCookie()
          .map((value) => value.split(';')[0])
          .join('; ')
      }
      function socket(
        transport: 'polling' | 'websocket',
        cookie: string,
        id: string,
        from = origin,
      ) {
        const client = io(origin, {
          transports: [transport],
          extraHeaders: { cookie, origin: from },
          auth: { userId: id },
          autoConnect: false,
          reconnection: false,
          forceNew: true,
          timeout: 3000,
        })
        sockets.push(client)
        return client
      }
      const [firstCookie, secondCookie] = await Promise.all([login(), login()])
      expect(await connect(socket('websocket', firstCookie, crypto.randomUUID()))).toBe(
        'Unauthorized',
      )
      expect(
        await connect(socket('polling', firstCookie, userId, 'https://example.invalid')),
      ).not.toBe('connected')
      expect(await connect(socket('polling', '', userId))).not.toBe('connected')
      const first = socket('websocket', firstCookie, userId)
      const second = socket('polling', secondCookie, userId)
      const hints: unknown[][] = []
      for (const client of [first, second])
        client.on('workspace:changed', (...args: unknown[]) => hints.push(args))
      expect(await Promise.all([connect(first), connect(second)])).toEqual([
        'connected',
        'connected',
      ])
      const nextHint = (client: Socket) =>
        new Promise<void>((resolve) => client.once('workspace:changed', resolve))
      const signals = Promise.all([nextHint(first), nextHint(second)])
      const writer = await pool.connect()
      try {
        await writer.query('BEGIN')
        await writer.query("UPDATE teachers SET name = 'Rolled back sync name' WHERE id = $1", [
          teacherId,
        ])
        await Bun.sleep(100)
        expect(hints).toEqual([])
        await writer.query('ROLLBACK')
      } finally {
        writer.release()
      }
      await pool.query(
        "INSERT INTO website_diagnostics (actor_id, event_id, journey_id, role, event) VALUES ($1, gen_random_uuid(), gen_random_uuid(), 'teacher', '{}')",
        [userId],
      )
      await Bun.sleep(150)
      expect(hints).toEqual([])
      await pool.query("UPDATE teachers SET name = 'Committed sync name' WHERE id = $1", [
        teacherId,
      ])
      await signals
      expect(hints).toEqual([[], []])

      await pool.query('SELECT pg_terminate_backend($1, 2000)', [firstPid])
      expect(await listenerId()).toBeUndefined()
      const recovered = Promise.all([nextHint(first), nextHint(second)])
      await pool.query("UPDATE teachers SET name = 'Recovered sync name' WHERE id = $1", [
        teacherId,
      ])
      await recovered
      const recoveredPid = await listenerId()
      expect(recoveredPid).toBeDefined()
      expect(recoveredPid).not.toBe(firstPid)
      const profile = await fetch(`${origin}/api/me`, {
        headers: { cookie: firstCookie, 'x-user-id': userId },
      })
      expect((await profile.json()).account.name).toBe('Recovered sync name')
      first.disconnect()
      await pool.query('UPDATE app_accounts SET active = false WHERE auth_user_id = $1', [userId])
      expect(await connect(first)).toBe('Unauthorized')
    } finally {
      for (const socket of sockets) socket.disconnect()
      server?.child.kill()
      if (userId) {
        await pool.query('DELETE FROM app_accounts WHERE auth_user_id = $1', [userId])
        await pool.query('DELETE FROM teachers WHERE id = $1', [teacherId])
        await pool.query('DELETE FROM neon_auth.session WHERE "userId" = $1', [userId])
        await pool.query('DELETE FROM neon_auth.account WHERE "userId" = $1', [userId])
        await pool.query('DELETE FROM neon_auth."user" WHERE id = $1', [userId])
      }
      await pool.end()
    }
  },
  20000,
)
