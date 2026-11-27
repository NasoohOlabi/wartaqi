import { expect, test } from 'bun:test'
import { Pool } from 'pg'
import { io, type Socket } from 'socket.io-client'
import { z } from 'zod'
import { createIdentity } from './integration-identity'
import { startIntegrationServer } from './integration-server'

// Only use a disposable, migrated database.
const databaseUrl = process.env.TEST_RAILWAY_DATABASE_URL
const rustBinary = process.env.TEST_RUST_SYNC_BINARY
const implementations = rustBinary ? ['Bun', 'Rust'] : ['Bun']
if (rustBinary && !databaseUrl) throw new Error('Rust parity requires TEST_RAILWAY_DATABASE_URL')
const integration = (databaseUrl ? test : test.skip).each(implementations)

async function startRustRelay(binary: string, databaseUrl: string, bunOrigin: string) {
  const child = Bun.spawn([binary], {
    env: {
      ...process.env,
      PORT: '0',
      BUN_ORIGIN: bunOrigin,
      WORKSPACE_ORIGIN: bunOrigin,
      DATABASE_URL: databaseUrl,
    },
    stdout: 'pipe',
    stderr: 'inherit',
  })
  const deadline = AbortSignal.timeout(10000)
  try {
    const ready = async () => {
      let output = ''
      for await (const chunk of child.stdout.pipeThrough(new TextDecoderStream())) {
        output += chunk
        const match = /^Listening on (http:\/\/127\.0\.0\.1:\d+)\r?$/m.exec(output)
        if (match) return match[1]
      }
      throw new Error('Rust relay exited before publishing its address')
    }
    const origin = await Promise.race([
      ready(),
      new Promise<never>((_, reject) => {
        deadline.addEventListener('abort', () => reject(deadline.reason), { once: true })
      }),
    ])
    const response = await fetch(`${origin}/health`, { signal: deadline })
    expect(response.status).toBe(200)
    expect(await response.json()).toEqual({ ok: true })
    return { origin, child }
  } catch (error) {
    child.kill()
    await child.exited
    throw error
  }
}

async function connect(socket: Socket) {
  const result = Promise.withResolvers<string>()
  socket.once('connect', () => result.resolve('connected'))
  socket.once('connect_error', (error) => result.resolve(error.message))
  socket.connect()
  return result.promise
}

integration(
  '%s sockets deliver committed changes and recover a terminated listener',
  async (implementation) => {
    if (!databaseUrl) throw new Error('Disposable database required')
    const target = new URL(databaseUrl)
    target.searchParams.set('application_name', `sync-test-${crypto.randomUUID()}`)
    const pool = new Pool({ connectionString: databaseUrl })
    const teacherId = crypto.randomUUID()
    const password = 'synthetic-test-password'
    const phone = '+12025550419'
    const sockets: Socket[] = []
    let server: Awaited<ReturnType<typeof startIntegrationServer>> | undefined
    let relay: Awaited<ReturnType<typeof startRustRelay>> | undefined
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
      server = await startIntegrationServer(implementation === 'Bun' ? target.href : databaseUrl)
      const { origin } = server
      if (implementation === 'Rust' && rustBinary) {
        relay = await startRustRelay(rustBinary, target.href, origin)
      }
      const socketOrigin = relay?.origin ?? origin
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
        options: { upgrade?: boolean; extraAuth?: object; omitOrigin?: boolean } = {},
      ) {
        const client = io(socketOrigin, {
          transports: options.upgrade ? ['polling', 'websocket'] : [transport],
          extraHeaders: {
            ...(cookie ? { cookie } : {}),
            ...(options.omitOrigin ? {} : { origin }),
          },
          auth: { userId: id, ...options.extraAuth },
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
        await connect(
          socket('websocket', firstCookie, userId, { extraAuth: { unexpected: true } }),
        ),
      ).toBe('Unauthorized')
      expect(await connect(socket('websocket', firstCookie, ''))).toBe('Unauthorized')
      expect(await connect(socket('websocket', firstCookie, 'x'.repeat(129)))).toBe('Unauthorized')
      const pollingUrl = `${socketOrigin}/socket.io/?EIO=4&transport=polling`
      const forbiddenHeaders: Record<string, string>[] = [
        { cookie: firstCookie, origin: 'https://example.invalid' },
        { cookie: firstCookie, origin, 'sec-fetch-site': 'same-site' },
      ]
      for (const headers of forbiddenHeaders) {
        const denied = await fetch(pollingUrl, { headers })
        expect(denied.status).toBe(403)
        expect(await denied.text()).toBe('Forbidden')
      }
      const missingCookie = await fetch(pollingUrl)
      expect(missingCookie.status).toBe(403)
      expect(await missingCookie.json()).toEqual({ code: 4, message: {} })

      await pool.query(
        'UPDATE app_accounts SET invitation_request_id = $2, invitation_requires_setup = true WHERE auth_user_id = $1',
        [userId, crypto.randomUUID()],
      )
      expect(await connect(socket('websocket', firstCookie, userId))).toBe('Unauthorized')
      await pool.query(
        'UPDATE app_accounts SET invitation_request_id = NULL, invitation_requires_setup = false WHERE auth_user_id = $1',
        [userId],
      )

      const first = socket('websocket', firstCookie, userId)
      const second = socket('polling', secondCookie, userId, { omitOrigin: true })
      const upgrading = socket('polling', firstCookie, userId, { upgrade: true })
      const upgraded = Promise.withResolvers<string>()
      upgrading.io.once('open', () => {
        const engine = upgrading.io.engine
        if (engine.transport.name === 'websocket') upgraded.resolve(engine.transport.name)
        else engine.once('upgrade', (transport) => upgraded.resolve(transport.name))
      })
      expect(await Promise.all([connect(first), connect(second)])).toEqual([
        'connected',
        'connected',
      ])
      expect(await connect(upgrading)).toBe('connected')
      expect(await upgraded.promise).toBe('websocket')
      const clients = [first, second, upgrading]
      const hints: unknown[][] = []
      for (const client of clients)
        client.on('workspace:changed', (...args: unknown[]) => hints.push(args))
      const nextHint = (client: Socket) =>
        new Promise<void>((resolve) => client.once('workspace:changed', resolve))
      const signals = Promise.all(clients.map(nextHint))
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
      expect(hints).toEqual([[], [], []])

      const recovered = Promise.all(clients.map(nextHint))
      await pool.query('SELECT pg_terminate_backend($1, 2000)', [firstPid])
      expect(await listenerId()).toBeUndefined()
      await pool.query("UPDATE teachers SET name = 'Recovered sync name' WHERE id = $1", [
        teacherId,
      ])
      await recovered
      expect(hints.length).toBeGreaterThanOrEqual(6)
      expect(hints.every((args) => args.length === 0)).toBe(true)
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
      relay?.child.kill()
      server?.child.kill()
      await Promise.all([relay?.child.exited, server?.child.exited])
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
