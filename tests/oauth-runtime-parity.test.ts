import { createHash, randomBytes, randomInt } from 'node:crypto'
import { exportJWK, generateKeyPair, SignJWT } from 'jose'
import { Pool } from 'pg'
import { expect, test } from 'vitest-runner'
import { z } from 'zod'
import { createIdentity } from './integration-identity'
import { startNodeIntegrationServer } from './integration-server'
import { startRustBackend } from './rust-backend-server'

const database = process.env.TEST_RAILWAY_DATABASE_URL
const binary = process.env.TEST_RUST_WEB_BINARY
const integration = database && binary ? test : test.skip
const clientSchema = z.object({ client_id: z.string(), client_secret: z.string() })
const redirectSchema = z.object({ url: z.string() })
const tokenSchema = z.object({ access_token: z.string(), refresh_token: z.string() })

async function verifyRefreshDuringSessionDeletion(
  pool: Pool,
  user: string,
  startRefresh: () => Promise<Response>,
) {
  const blocker = await pool.connect()
  let refresh: Promise<Response> | undefined
  try {
    await blocker.query('BEGIN')
    const held = await blocker.query<{ pid: number }>(
      'SELECT pg_backend_pid() AS pid FROM neon_auth.session WHERE "userId"=$1 FOR UPDATE',
      [user],
    )
    const pid = held.rows[0]?.pid
    if (held.rowCount !== 1 || pid === undefined) throw new Error('Expected one fixture session')
    refresh = startRefresh()
    await expect
      .poll(
        async () => {
          const result = await pool.query<{ waiting: boolean }>(
            `SELECT EXISTS (
               SELECT 1 FROM pg_stat_activity
               WHERE application_name='wartaq-rust-api' AND $1 = ANY(pg_blocking_pids(pid))
             ) AS waiting`,
            [pid],
          )
          return result.rows[0]?.waiting
        },
        { interval: 25, timeout: 5000 },
      )
      .toBe(true)
    await blocker.query('DELETE FROM neon_auth.session WHERE "userId"=$1', [user])
    await blocker.query('COMMIT')
    const response = await refresh
    expect(response.status).toBe(400)
    expect(await response.json()).toMatchObject({ error: 'invalid_grant' })
  } finally {
    await blocker.query('ROLLBACK').finally(() => blocker.release())
    await refresh?.catch(() => undefined)
  }
}

integration(
  'Rust and Better Auth can use each other’s stored OAuth grants',
  async () => {
    if (!database || !binary) throw new Error('A disposable database and Rust binary are required')
    const secret = randomBytes(32).toString('hex')
    const legacy = await startNodeIntegrationServer(database, secret)
    const pool = new Pool({ connectionString: database })
    const clients: string[] = []
    const teacher = crypto.randomUUID()
    let user = ''
    let native: Awaited<ReturnType<typeof startRustBackend>> | undefined
    try {
      native = await startRustBackend(
        binary,
        database,
        { hybridOrigin: legacy.origin, port: '0', native: true },
        secret,
        legacy.origin,
      )
      const resource = legacy.origin + '/mcp'
      const issuer = legacy.origin + '/api/auth'
      for (const origin of [legacy.origin, native.origin]) {
        const rejected = await fetch(
          origin + '/api/auth/oauth2/authorize?client_id=unknown-client&response_type=code',
          { headers: { accept: 'application/json' } },
        )
        const destination = new URL(redirectSchema.parse(await rejected.json()).url, origin)
        expect(destination.pathname).toBe('/api/auth/error')
        expect(destination.searchParams.get('error')).toBe('invalid_client')
        const page = await fetch(origin + destination.pathname + destination.search)
        expect(page.status).toBe(200)
        expect(page.headers.get('cache-control')).toBe('no-store')
        expect(page.headers.get('content-type')).toMatch(/^text\/html/)
        expect(await page.text()).toContain('client not found')
      }
      const markup = '<b>synthetic error</b>'
      const escaped = await fetch(
        native.origin + '/api/auth/error?error_description=' + encodeURIComponent(markup),
      )
      const html = await escaped.text()
      expect(html).toContain('&lt;b&gt;synthetic error&lt;/b&gt;')
      expect(html).not.toContain(markup)
      const redirect = 'http://127.0.0.1:39386/callback'
      const phone = `+1202${randomInt(0, 10_000_000).toString().padStart(7, '0')}`
      const password = randomBytes(32).toString('base64url')
      user = await createIdentity(pool, 'Synthetic OAuth administrator', password)
      await pool.query(
        "INSERT INTO teachers (id, auth_user_id, name, access_mode) VALUES ($1,$2,'OAuth fixture','read_write')",
        [teacher, user],
      )
      await pool.query(
        "INSERT INTO app_accounts (auth_user_id, role, verified_phone, teacher_id) VALUES ($1,'administrator',$2,$3)",
        [user, phone, teacher],
      )
      const json = (
        origin: string,
        path: string,
        body: unknown,
        headers?: Record<string, string>,
      ) =>
        fetch(origin + path, {
          method: 'POST',
          headers: { 'content-type': 'application/json', ...headers },
          body: JSON.stringify(body),
          redirect: 'manual',
        })
      const form = (origin: string, endpoint: string, fields: Record<string, string>) =>
        fetch(origin + '/api/auth/oauth2/' + endpoint, {
          method: 'POST',
          body: new URLSearchParams(fields),
        })
      const login = await json(
        legacy.origin,
        '/api/auth/sign-in/phone',
        { phone, password },
        {
          origin: legacy.origin,
        },
      )
      expect(login.status).toBe(200)
      const cookie = login.headers
        .getSetCookie()
        .find((value) => value.startsWith('better-auth.session_token='))
        ?.split(';')[0]
      if (!cookie) throw new Error('The Better Auth session cookie was missing')
      for (const [issuerOrigin, redeemOrigin] of [
        [legacy.origin, native.origin],
        [native.origin, legacy.origin],
      ]) {
        const registered = await json(issuerOrigin, '/api/auth/oauth2/register', {
          redirect_uris: [redirect],
          token_endpoint_auth_method: 'client_secret_post',
          grant_types: ['authorization_code', 'refresh_token'],
        })
        expect(registered.status).toBe(201)
        const credentials = clientSchema.parse(await registered.json())
        clients.push(credentials.client_id)
        const verifier = randomBytes(32).toString('base64url')
        const authorize = new URL('/api/auth/oauth2/authorize', issuerOrigin)
        authorize.search = new URLSearchParams({
          client_id: credentials.client_id,
          response_type: 'code',
          redirect_uri: redirect,
          scope: 'mcp:read offline_access',
          resource,
          code_challenge_method: 'S256',
          code_challenge: createHash('sha256').update(verifier).digest('base64url'),
        }).toString()
        const prompt = await fetch(authorize, { headers: { accept: 'application/json', cookie } })
        expect(prompt.status).toBe(200)
        const consent = new URL(redirectSchema.parse(await prompt.json()).url, legacy.origin)
        expect(consent.pathname).toBe('/mcp/consent')
        const approved = await json(
          issuerOrigin,
          '/api/auth/oauth2/consent',
          {
            accept: true,
            oauth_query: consent.search.slice(1),
          },
          { origin: legacy.origin, cookie, 'x-user-id': user },
        )
        expect(approved.status).toBe(200)
        const code = new URL(redirectSchema.parse(await approved.json()).url).searchParams.get(
          'code',
        )
        if (!code) throw new Error('The callback did not include an authorization code')
        const exchange = {
          ...credentials,
          grant_type: 'authorization_code',
          code,
          redirect_uri: redirect,
          resource,
          code_verifier: verifier,
        }
        const wrongVerifier = 'A'.repeat(43)
        expect(
          (await form(redeemOrigin, 'token', { ...exchange, code_verifier: wrongVerifier })).status,
        ).toBe(400)
        const issued = await form(redeemOrigin, 'token', exchange)
        expect(issued.status).toBe(200)
        const grant = tokenSchema.parse(await issued.json())
        const inspect = await form(issuerOrigin, 'introspect', {
          ...credentials,
          token: grant.access_token,
        })
        expect(await inspect.json()).toMatchObject({ active: true, sub: user, aud: resource })
        const renewed = await form(issuerOrigin, 'token', {
          ...credentials,
          grant_type: 'refresh_token',
          refresh_token: grant.refresh_token,
          resource,
        })
        expect(renewed.status).toBe(200)
        const rotated = tokenSchema.parse(await renewed.json())
        expect(rotated.refresh_token).not.toBe(grant.refresh_token)
        expect(
          (await form(redeemOrigin, 'revoke', { ...credentials, token: rotated.access_token }))
            .status,
        ).toBe(200)
        const revoked = await form(issuerOrigin, 'introspect', {
          ...credentials,
          token: rotated.access_token,
        })
        expect(await revoked.json()).toEqual({ active: false })
        if (issuerOrigin === native.origin) {
          await verifyRefreshDuringSessionDeletion(pool, user, () =>
            form(issuerOrigin, 'token', {
              ...credentials,
              grant_type: 'refresh_token',
              refresh_token: rotated.refresh_token,
              resource,
            }),
          )
        }
      }

      const { privateKey, publicKey } = await generateKeyPair('ES512')
      const registration = await json(native.origin, '/api/auth/oauth2/register', {
        redirect_uris: [redirect],
        token_endpoint_auth_method: 'private_key_jwt',
        jwks: { keys: [await exportJWK(publicKey)] },
      })
      expect(registration.status).toBe(201)
      const { client_id: client } = clientSchema
        .pick({ client_id: true })
        .parse(await registration.json())
      clients.push(client)
      const assertion = await new SignJWT({})
        .setProtectedHeader({ alg: 'ES512' })
        .setIssuedAt()
        .setExpirationTime('1m')
        .setIssuer(client)
        .setSubject(client)
        .setAudience(issuer)
        .setJti(crypto.randomUUID())
        .sign(privateKey)
      const fields = {
        client_assertion_type: 'urn:ietf:params:oauth:client-assertion-type:jwt-bearer',
        client_assertion: assertion,
        token: 'unknown',
      }
      const allowed = await form(native.origin, 'introspect', fields)
      expect(allowed.status).toBe(200)
      expect(await allowed.json()).toEqual({ active: false })
      expect((await form(native.origin, 'introspect', fields)).status).toBe(401)
    } finally {
      await Promise.all([native?.close(), legacy.close()])
      try {
        for (const table of [
          'oauthAccessToken',
          'oauthRefreshToken',
          'oauthConsent',
          'oauthClientResource',
          'oauthClient',
        ]) {
          await pool.query(`DELETE FROM neon_auth."${table}" WHERE "clientId" = ANY($1)`, [clients])
        }
        if (user) {
          await pool.query('DELETE FROM app_accounts WHERE auth_user_id=$1', [user])
          await pool.query('DELETE FROM teachers WHERE id=$1', [teacher])
          await pool.query('DELETE FROM neon_auth."user" WHERE id=$1', [user])
        }
      } finally {
        await pool.end()
      }
    }
  },
  30_000,
)
