import { afterAll, expect, test } from 'bun:test'
import { randomBytes } from 'node:crypto'
import { Pool } from 'pg'
import { z } from 'zod'
import { neonStudentAttendanceSchema } from '../shared/neon'
import { revokeTelegramGrants } from '../shared/telegram-grants'
import { createIdentity } from './integration-identity'
import { startIntegrationServer } from './integration-server'

const databaseUrl = process.env.TEST_RAILWAY_DATABASE_URL
const integration = databaseUrl ? test : test.skip
const secret = randomBytes(32).toString('hex')
let pool: Pool | undefined
let server: ReturnType<typeof Bun.spawn> | undefined
const ids: string[] = []
const studentId = crypto.randomUUID()
const teacherId = crypto.randomUUID()
const courseId = crypto.randomUUID()
const enrollmentId = crypto.randomUUID()
const eventId = crypto.randomUUID()
const teacherEventId = crypto.randomUUID()
const otherStudentId = crypto.randomUUID()
const otherEnrollmentId = crypto.randomUUID()

afterAll(async () => {
  server?.kill()
  if (!pool) return
  const db = await pool.connect()
  try {
    for (const id of ids) {
      await revokeTelegramGrants(db, id)
      await db.query('DELETE FROM app_accounts WHERE auth_user_id = $1', [id])
    }
  } finally {
    db.release()
  }
  await pool.query('DELETE FROM events WHERE id = ANY($1::uuid[])', [[eventId, teacherEventId]])
  await pool.query('DELETE FROM course_enrollments WHERE id = ANY($1::uuid[])', [
    [enrollmentId, otherEnrollmentId],
  ])
  await pool.query('DELETE FROM courses WHERE id = $1', [courseId])
  await pool.query('DELETE FROM students WHERE id = ANY($1::uuid[])', [[studentId, otherStudentId]])
  await pool.query('DELETE FROM teachers WHERE id = $1', [teacherId])
  for (const id of ids) await pool.query('DELETE FROM neon_auth."user" WHERE id = $1', [id])
  await pool.end()
})

integration('phone grants follow verified Student and Teacher access and revoke', async () => {
  if (!databaseUrl) throw new Error('Disposable database required')
  pool = new Pool({ connectionString: databaseUrl })
  const previousSecret = process.env.TELEGRAM_WORKER_SECRET
  process.env.TELEGRAM_WORKER_SECRET = secret
  const started = await startIntegrationServer(databaseUrl).finally(() => {
    if (previousSecret === undefined) delete process.env.TELEGRAM_WORKER_SECRET
    else process.env.TELEGRAM_WORKER_SECRET = previousSecret
  })
  server = started.child
  const password = randomBytes(32).toString('hex')
  const user = await createIdentity(pool, 'Synthetic student', password)
  const staff = await createIdentity(pool, 'Synthetic staff', randomBytes(32).toString('hex'))
  ids.push(user, staff)
  await pool.query(
    "INSERT INTO students (id, name, whatsapp) VALUES ($1, 'Synthetic student', '+12025550791')",
    [studentId],
  )
  await pool.query(
    "INSERT INTO app_accounts (auth_user_id, role, student_id, verified_phone, invitation_requires_setup, invitation_request_id) VALUES ($1, 'student', $2, $3, true, gen_random_uuid())",
    [user, studentId, '+963995120034'],
  )
  await pool.query(
    "INSERT INTO teachers (id, auth_user_id, name, access_mode) VALUES ($1, $2, 'Synthetic staff', 'read_write')",
    [teacherId, staff],
  )
  await pool.query(
    "INSERT INTO app_accounts (auth_user_id, role, teacher_id, verified_phone) VALUES ($1, 'teacher', $2, $3)",
    [staff, teacherId, '+963995120035'],
  )
  await pool.query("INSERT INTO courses (id, name) VALUES ($1, 'Synthetic course')", [courseId])
  await pool.query(
    'INSERT INTO course_enrollments (id, course_id, student_id) VALUES ($1, $2, $3)',
    [enrollmentId, courseId, studentId],
  )
  await pool.query(
    "INSERT INTO events (id, teacher_id, student_id, enrollment_id, actor, client_time, payload) VALUES ($1, $2, $3, $4, 'Synthetic staff', now(), $5)",
    [
      eventId,
      teacherId,
      studentId,
      enrollmentId,
      { kind: 'attendance', studentId, enrollmentId, date: '2026-11-20', status: 'present' },
    ],
  )
  const post = (body: unknown, authorization = `Bearer ${secret}`) =>
    fetch(`${started.origin}/api/telegram/student`, {
      method: 'POST',
      headers: { authorization, 'content-type': 'application/json' },
      body: JSON.stringify(body),
    })
  expect((await post({ action: 'login', phone: '0995120034' }, 'Bearer invalid')).status).toBe(401)
  expect((await post({ action: 'login', phone: '+12025550791' })).status).toBe(403)
  for (const phone of ['0995120035', '+963 995 120 035']) {
    expect((await post({ action: 'login', phone })).status).toBe(403)
  }
  const signin = await fetch(`${started.origin}/api/auth/sign-in/phone`, {
    method: 'POST',
    headers: { origin: started.origin, 'content-type': 'application/json' },
    body: JSON.stringify({ phone: '0995 120 034', password }),
  })
  expect(signin.status).toBe(200)
  const cookie = signin.headers
    .getSetCookie()
    .find((value) => value.startsWith('better-auth.session_token='))
    ?.split(';')[0]
  expect(cookie).toBeDefined()
  const browserAttendance = await fetch(`${started.origin}/api/student/attendance`, {
    headers: { cookie: cookie ?? '', 'x-user-id': user },
  })
  expect(browserAttendance.status).toBe(403)
  const response = await post({ action: 'login', phone: '+963 995 120 034' })
  expect(response.status).toBe(200)
  const { token } = z.object({ token: z.string() }).parse(await response.json())
  const attendance = await post({ action: 'attendance', token })
  expect(attendance.status).toBe(200)
  expect(neonStudentAttendanceSchema.parse(await attendance.json())).toEqual({
    student: { studentId, name: 'Synthetic student', group: '' },
    attendance: [
      { date: '2026-11-20', status: 'present', enrollmentId, course: 'Synthetic course' },
    ],
  })
  await pool.query('UPDATE students SET teacher_id = $2 WHERE id = $1', [studentId, teacherId])
  await pool.query("INSERT INTO students (id, name) VALUES ($1, 'Other student')", [otherStudentId])
  await pool.query(
    'INSERT INTO course_enrollments (id, course_id, student_id) VALUES ($1, $2, $3)',
    [otherEnrollmentId, courseId, otherStudentId],
  )
  const teacherLogin = await post({ action: 'login-phone', phone: '0995120035' })
  expect(teacherLogin.status).toBe(200)
  const teacherGrant = z
    .object({ token: z.string(), role: z.literal('teacher') })
    .parse(await teacherLogin.json()).token
  const rpc = (grant: string, method: string, params: unknown = {}, workerSecret = secret) =>
    fetch(`${started.origin}/api/telegram/teacher-mcp`, {
      method: 'POST',
      headers: {
        authorization: `Bearer ${workerSecret}`,
        'x-telegram-grant': grant,
        'content-type': 'application/json',
        accept: 'application/json, text/event-stream',
      },
      body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }),
    })
  expect((await rpc(teacherGrant, 'tools/list', {}, 'invalid')).status).toBe(401)
  expect((await rpc(token, 'tools/list')).status).toBe(403)
  expect(
    (
      await fetch(`${started.origin}/mcp`, {
        method: 'POST',
        headers: { authorization: `Bearer ${teacherGrant}` },
      })
    ).status,
  ).toBe(401)
  const list = await rpc(teacherGrant, 'tools/list')
  expect(list.status).toBe(200)
  const tools = z
    .object({ result: z.object({ tools: z.array(z.object({ name: z.string() })) }) })
    .parse(await list.json())
    .result.tools.map((tool) => tool.name)
  expect(tools).toContain('get_notebook')
  expect(tools).toContain('sync_attendance')
  expect(tools).not.toContain('grant_account')
  const notebook = await rpc(teacherGrant, 'tools/call', { name: 'get_notebook', arguments: {} })
  expect(notebook.status).toBe(200)
  const notebookBody = JSON.stringify(await notebook.json())
  expect(notebookBody).toContain(studentId)
  expect(notebookBody).not.toContain(otherStudentId)
  const event = {
    id: teacherEventId,
    clientTime: new Date().toISOString(),
    payload: { kind: 'attendance', studentId, enrollmentId, date: '2026-11-21', status: 'present' },
  }
  const written = await rpc(teacherGrant, 'tools/call', {
    name: 'append_events',
    arguments: { events: [event] },
  })
  expect(JSON.stringify(await written.json())).not.toContain('isError')
  expect(
    (
      await pool.query('SELECT 1 FROM events WHERE id = $1 AND student_id = $2', [
        teacherEventId,
        studentId,
      ])
    ).rowCount,
  ).toBe(1)
  const deniedEvent = {
    ...event,
    id: crypto.randomUUID(),
    payload: { ...event.payload, studentId: otherStudentId, enrollmentId: otherEnrollmentId },
  }
  const deniedWrite = await rpc(teacherGrant, 'tools/call', {
    name: 'append_events',
    arguments: { events: [deniedEvent] },
  })
  expect(JSON.stringify(await deniedWrite.json())).toContain('isError')
  expect((await pool.query('SELECT 1 FROM events WHERE id = $1', [deniedEvent.id])).rowCount).toBe(
    0,
  )
  const profile = await rpc(teacherGrant, 'tools/call', { name: 'get_profile', arguments: {} })
  expect(profile.status).toBe(200)
  expect(JSON.stringify(await profile.json())).toContain('Synthetic staff')
  const renamed = await rpc(teacherGrant, 'tools/call', {
    name: 'update_profile',
    arguments: { input: { name: 'Updated synthetic staff' } },
  })
  expect(JSON.stringify(await renamed.json())).toContain('Updated synthetic staff')
  await pool.query(
    'UPDATE app_accounts SET invitation_requires_setup = true, invitation_request_id = gen_random_uuid() WHERE auth_user_id = $1',
    [staff],
  )
  expect((await post({ action: 'login-phone', phone: '0995120035' })).status).toBe(403)
  const pending = await rpc(teacherGrant, 'tools/call', { name: 'get_notebook', arguments: {} })
  expect(JSON.stringify(await pending.json())).toContain('isError')
  await pool.query(
    'UPDATE app_accounts SET invitation_requires_setup = false WHERE auth_user_id = $1',
    [staff],
  )
  await pool.query('UPDATE teachers SET active = false WHERE id = $1', [teacherId])
  expect((await rpc(teacherGrant, 'tools/list')).status).toBe(403)
  await pool.query('UPDATE teachers SET active = true WHERE id = $1', [teacherId])
  await pool.query("UPDATE app_accounts SET role = 'administrator' WHERE auth_user_id = $1", [
    staff,
  ])
  expect((await post({ action: 'login-phone', phone: '0995120035' })).status).toBe(403)
  expect((await rpc(teacherGrant, 'tools/list')).status).toBe(403)
  await pool.query("UPDATE app_accounts SET role = 'teacher' WHERE auth_user_id = $1", [staff])
  await pool.query('UPDATE app_accounts SET active = false WHERE auth_user_id = $1', [staff])
  expect((await rpc(teacherGrant, 'tools/list')).status).toBe(403)
  await pool.query(
    'UPDATE app_accounts SET active = true, verified_phone = $2 WHERE auth_user_id = $1',
    [staff, '+963995120036'],
  )
  expect((await rpc(teacherGrant, 'tools/list')).status).toBe(403)
  expect((await post({ action: 'unlink', token: teacherGrant })).status).toBe(200)
  expect((await rpc(teacherGrant, 'tools/list')).status).toBe(401)
  await pool.query('UPDATE app_accounts SET active = false WHERE auth_user_id = $1', [user])
  expect((await post({ action: 'attendance', token })).status).toBe(403)
  await pool.query(
    "UPDATE app_accounts SET active = true, student_id = $2, verified_phone = '+12025550793' WHERE auth_user_id = $1",
    [user, studentId],
  )
  expect((await post({ action: 'attendance', token })).status).toBe(403)
  expect((await post({ action: 'unlink', token })).status).toBe(200)
  expect((await post({ action: 'attendance', token })).status).toBe(403)
})
