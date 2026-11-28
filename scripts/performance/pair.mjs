import { createHash, randomBytes, randomUUID } from 'node:crypto'
import { readFile, writeFile } from 'node:fs/promises'
import { deepStrictEqual, equal, ok } from 'node:assert/strict'
import { get } from 'node:http'

const [label, ...ports] = process.argv.slice(2)
if (
  !/^[a-z0-9-]+$/.test(label ?? '') ||
  ports.length !== 2 ||
  ports.some((port) => !/^\d+$/.test(port))
)
  throw new Error('Usage: node scripts/performance/pair.mjs LABEL CONTROL_PORT CANDIDATE_PORT')
const password = (await readFile('.artifacts/performance/fixture-password.local', 'utf8')).trim()
const log = []

function canonical(value, origin) {
  if (Array.isArray(value)) return value.map((item) => canonical(item, origin))
  if (typeof value === 'string') return value.replaceAll(origin, 'http://fixture.invalid')
  if (value !== null && typeof value === 'object')
    return Object.fromEntries(
      Object.entries(value)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, item]) => [key, canonical(item, origin)]),
    )
  return value
}

function client(port) {
  const origin = 'http://127.0.0.1:' + port
  const cookies = new Map()
  let user
  return async (path, body, status, project, authenticated = true) => {
    const response = await fetch(origin + path, {
      method: body === undefined ? 'GET' : 'POST',
      headers: {
        origin,
        ...(body !== undefined && { 'Content-Type': 'application/json' }),
        ...(authenticated && {
          cookie: [...cookies.values()].join('; '),
          ...(user && { 'X-User-Id': user }),
        }),
      },
      ...(body !== undefined && { body: JSON.stringify(body) }),
      redirect: 'manual',
      signal: AbortSignal.timeout(20000),
    })
    equal(response.status, status, path + ' status on ' + origin)
    const cookieFlags = response.headers
      .getSetCookie()
      .map((line) => {
        const [pair, ...flags] = line.split(';')
        const key = pair.split('=', 1)[0]
        if (pair === key + '=') cookies.delete(key)
        else cookies.set(key, pair)
        return flags
          .map((flag) =>
            flag
              .trim()
              .toLowerCase()
              .replace(/^expires=.*$/, 'expires'),
          )
          .sort()
      })
      .sort()
    const raw = await response.text()
    const data =
      response.headers.get('content-type')?.includes('application/json') && raw
        ? JSON.parse(raw)
        : raw
    if (path === '/api/auth/sign-in/phone' && status === 200) {
      ok(data.user?.id && cookies.size, 'Login establishes an identified cookie session')
      user = data.user.id
    }
    return { status, cookieFlags, value: canonical(project ? project(data) : data, origin) }
  }
}

const origins = ports.map(client)
const digest = (value) => createHash('sha256').update(JSON.stringify(value)).digest('hex')
async function request(path, body, status, project, authenticated) {
  const [control, candidate] = await Promise.all(
    origins.map((send) => send(path, body, status, project, authenticated)),
  )
  const controlDigest = digest(control)
  const candidateDigest = digest(candidate)
  log.push({
    path,
    method: body === undefined ? 'GET' : 'POST',
    status,
    controlDigest,
    candidateDigest,
  })
  deepStrictEqual(candidateDigest, controlDigest, path + ' canonical response')
  return control.value
}

async function oauthJourney(port, state, verifier) {
  const origin = `http://127.0.0.1:${port}`
  const resource = origin + '/mcp'
  const callbackUrl = 'https://pair.invalid/oauth-callback'
  const send = (path, options) =>
    fetch(new URL(path, origin), {
      ...options,
      redirect: 'manual',
      signal: AbortSignal.timeout(20000),
    })
  const json = (path, body, headers) =>
    send(path, {
      method: 'POST',
      headers: { ...headers, 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
  const registration = await json('/api/auth/oauth2/register', {
    client_name: 'Synthetic pair client',
    redirect_uris: [callbackUrl],
    token_endpoint_auth_method: 'none',
    grant_types: ['authorization_code', 'refresh_token'],
  })
  equal(registration.status, 201, 'Dynamic client registration')
  const { client_id } = await registration.json()
  ok(client_id)
  const authorize = new URL('/api/auth/oauth2/authorize', origin)
  authorize.search = new URLSearchParams({
    client_id,
    redirect_uri: callbackUrl,
    response_type: 'code',
    resource,
    scope: 'mcp:read offline_access',
    code_challenge: createHash('sha256').update(verifier).digest('base64url'),
    code_challenge_method: 'S256',
    state,
  })
  const login = await new Promise((resolve, reject) => {
    get(
      authorize,
      { headers: { accept: 'text/html' }, signal: AbortSignal.timeout(20000) },
      (response) => {
        response.resume()
        if (response.statusCode !== 302 || !response.headers.location) {
          reject(new Error('OAuth authorize did not redirect to the login page'))
          return
        }
        resolve(new URL(response.headers.location, origin))
      },
    ).once('error', reject)
  })
  equal(login.pathname, '/mcp/login')
  equal((await send(login)).status, 200, 'Signed login page')
  const signedIn = await json(
    '/api/auth/sign-in/phone',
    { phone: '+12025551001', password, rememberMe: false, oauth_query: login.search.slice(1) },
    { origin, 'x-forwarded-for': '192.0.2.29' },
  )
  equal(signedIn.status, 200, 'Phone sign-in with an OAuth continuation')
  const consent = new URL((await signedIn.json()).url, origin)
  equal(consent.pathname, '/mcp/consent')
  const cookie = signedIn.headers
    .getSetCookie()
    .map((line) => line.split(';', 1)[0])
    .join('; ')
  const session = await send('/api/auth/get-session?disableRefresh=true', { headers: { cookie } })
  const { user } = await session.json()
  const accepted = await json(
    '/api/auth/oauth2/consent',
    {
      accept: true,
      oauth_query: consent.search.slice(1),
    },
    { origin, cookie, 'x-user-id': user.id },
  )
  equal(accepted.status, 200, 'Consent under the signed-in account')
  const callback = new URL((await accepted.json()).url)
  equal(callback.origin + callback.pathname, callbackUrl)
  equal(callback.searchParams.get('state'), state)
  equal(callback.searchParams.get('iss'), origin + '/api/auth')
  const token = await send('/api/auth/oauth2/token', {
    method: 'POST',
    body: new URLSearchParams({
      client_id,
      grant_type: 'authorization_code',
      code: callback.searchParams.get('code'),
      code_verifier: verifier,
      redirect_uri: callbackUrl,
      resource,
    }),
  })
  equal(token.status, 200, 'PKCE authorization code exchange')
  const { access_token, refresh_token, scope } = await token.json()
  ok(access_token && refresh_token)
  const mcp = await json(
    '/mcp',
    {
      jsonrpc: '2.0',
      id: 1,
      method: 'initialize',
      params: {
        protocolVersion: '2025-11-25',
        capabilities: {},
        clientInfo: { name: 'pair', version: '1' },
      },
    },
    { authorization: `Bearer ${access_token}`, accept: 'application/json, text/event-stream' },
  )
  equal(mcp.status, 200, 'Scoped MCP initialization')
  const body = await mcp.text()
  const event = body.match(/^data: (.+)$/m)?.[1]
  const initialized = JSON.parse(event ?? body)
  return { scope, userId: user.id, version: initialized.result.protocolVersion }
}

try {
  await request('/health', undefined, 200)
  await request('/api/notebook', undefined, 401)
  await request('/.well-known/oauth-authorization-server/api/auth', undefined, 200)
  const teacherUser = await request(
    '/api/auth/sign-in/phone',
    { phone: '+12025551001', password },
    200,
    ({ user }) => user,
  )
  const teacher = await request('/api/me', undefined, 200)
  equal(teacher.account.role, 'teacher')
  await request('/api/auth/get-session', undefined, 200, ({ user }) => user)
  const device = await request('/api/device-accounts', undefined, 200)
  equal(device.accounts[0].userId, teacherUser.id)
  const notebook = await request('/api/notebook', undefined, 200)
  ok(notebook.events.length && notebook.enrollments.length)
  await request('/api/admin', undefined, 403)
  const entry = notebook.enrollments.find(
    (enrollment) =>
      enrollment.active &&
      notebook.courses.some((course) => course.id === enrollment.courseId && course.open),
  )
  ok(entry)
  const eventId = randomUUID()
  const attendance = await request(
    '/api/attendance/sync',
    {
      accountId: teacherUser.id,
      records: [
        {
          event: {
            id: eventId,
            actor: teacher.account.name,
            clientTime: '2026-11-22T12:00:00.000Z',
            receivedAt: '',
            payload: {
              kind: 'attendance',
              studentId: entry.studentId,
              enrollmentId: entry.id,
              date: '2026-11-22',
              status: 'present',
            },
          },
          expectedEventId: null,
        },
      ],
    },
    200,
  )
  equal(attendance.outcomes[0].kind, 'saved')
  await request('/api/auth/sign-out', {}, 200)
  await request('/api/notebook', undefined, 401)
  await request(
    '/api/auth/sign-in/phone',
    { phone: '+12025552001', password, rememberMe: false },
    200,
    ({ user }) => user,
  )
  const student = await request('/api/student/attendance', undefined, 200)
  ok(student.attendance.length)
  await request('/api/notebook', undefined, 403)
  await request('/api/auth/sign-out', {}, 200)
  await request(
    '/api/auth/sign-in/phone',
    { phone: '+12025551000', password },
    200,
    ({ user }) => user,
  )
  await request(
    '/api/auth/sign-in/phone',
    { phone: '+12025551001', password },
    429,
    undefined,
    false,
  )
  const administrator = await request('/api/admin', undefined, 200)
  ok(administrator.accounts.length && administrator.students.length)
  const courseId = randomUUID()
  await request(
    '/api/admin/courses',
    { id: courseId, name: 'Pair fixture course', open: true },
    200,
  )
  await request('/api/admin/courses/status', { id: courseId, open: false }, 200)
  const closed = await request('/api/notebook', undefined, 200, ({ courses }) =>
    courses.find(({ id }) => id === courseId),
  )
  equal(closed.open, false)
  const invitation = {
    requestId: randomUUID(),
    people: [
      {
        firstName: 'Pair',
        lastName: 'Fixture',
        role: 'teacher',
        phone: '+12025550110',
        phoneConfirmed: true,
      },
    ],
  }
  await request('/api/admin/accounts/invite', invitation, 200, ({ invitations }) =>
    invitations.map(({ temporaryPassword, ...person }) => {
      ok(temporaryPassword.length >= 8)
      return person
    }),
  )
  await request('/api/admin/accounts/invite', invitation, 409)
  const state = randomUUID()
  const verifier = randomBytes(32).toString('base64url')
  const [control, candidate] = await Promise.all(
    ports.map((port) => oauthJourney(port, state, verifier)),
  )
  deepStrictEqual(candidate, control, 'OAuth and MCP journey')
  log.push({
    journey: 'oauth-mcp',
    controlDigest: digest(control),
    candidateDigest: digest(candidate),
  })
  console.log('Matched 24 HTTP responses and the OAuth/MCP journey.')
} finally {
  await writeFile(
    '.artifacts/performance/' + label + '-pair.json',
    JSON.stringify(log, null, 2) + '\n',
  )
}
