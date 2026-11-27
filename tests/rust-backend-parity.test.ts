import { expect, test } from 'bun:test'
import { createHash, randomBytes } from 'node:crypto'
import { Pool } from 'pg'
import { z } from 'zod'
import { createIdentity } from './integration-identity'
import { startIntegrationServer } from './integration-server'
import { startRustBackend } from './rust-backend-server'

const databaseUrl = process.env.TEST_RAILWAY_DATABASE_URL
const binary = process.env.TEST_RUST_SYNC_BINARY
const parity = databaseUrl && binary ? test : test.skip

parity(
  'Rust and Bun return identical role-scoped reads and write outcomes',
  async () => {
    if (!databaseUrl || !binary)
      throw new Error('A disposable database and Rust binary are required')
    const pool = new Pool({ connectionString: databaseUrl })
    let bun: Awaited<ReturnType<typeof startIntegrationServer>> | undefined
    let rust: Awaited<ReturnType<typeof startRustBackend>> | undefined
    const uuid = () => crypto.randomUUID()
    const teacherId = uuid()
    const adminId = uuid()
    const studentId = uuid()
    const historicalId = uuid()
    const courseIds = Array.from({ length: 3 }, uuid)
    const enrollmentIds = Array.from({ length: 3 }, uuid)
    const users: string[] = []
    const receipts: string[] = []
    const telegramGrants: string[] = []
    const trace = randomBytes(9).toString('base64url')
    const password = 'synthetic-parity-password'
    try {
      const secret = randomBytes(32).toString('hex')
      const telegramWorkerSecret = randomBytes(32).toString('hex')
      bun = await startIntegrationServer(databaseUrl, secret, telegramWorkerSecret)
      const { origin } = bun
      rust = await startRustBackend(
        binary,
        databaseUrl,
        origin,
        secret,
        origin,
        telegramWorkerSecret,
      )
      const endpoints = [origin, rust.origin]
      for (const [role, id, phone] of [
        ['teacher', teacherId, '+12025550181'],
        ['administrator', adminId, '+12025550182'],
        ['student', studentId, '+12025550183'],
      ]) {
        const user = await createIdentity(pool, `Synthetic ${role}`, password)
        users.push(user)
        if (role === 'student') {
          await pool.query(
            "INSERT INTO students (id, teacher_id, name, group_name) VALUES ($1,$2,'Synthetic learner','Roster level')",
            [id, teacherId],
          )
        } else {
          await pool.query(
            "INSERT INTO teachers (id, auth_user_id, name, access_mode) VALUES ($1,$2,$3,'read_write')",
            [id, user, `Synthetic ${role}`],
          )
        }
        await pool.query(
          'INSERT INTO app_accounts (auth_user_id, role, teacher_id, student_id, verified_phone) VALUES ($1,$2,$3,$4,$5)',
          [user, role, role === 'student' ? null : id, role === 'student' ? id : null, phone],
        )
      }
      await pool.query(
        "INSERT INTO students (id,name,active) VALUES ($1,'Historical learner',false)",
        [historicalId],
      )
      for (const [index, name] of ['algebra', 'Álgebra', 'العربية'].entries()) {
        await pool.query('INSERT INTO courses (id,name) VALUES ($1,$2)', [courseIds[index], name])
        await pool.query(
          "INSERT INTO course_enrollments (id,course_id,student_id,level,practical,theory,notes,active) VALUES ($1,$2,$3,'',95,87.5,'Private grade note',$4)",
          [enrollmentIds[index], courseIds[index], studentId, index !== 2],
        )
      }
      const date = '2026-11-20'
      const early = uuid()
      const mark = (status: string | null, enrollmentId: string) => ({
        kind: 'attendance',
        studentId,
        date,
        status,
        enrollmentId,
      })
      const events = [
        { id: early, enrollment: enrollmentIds[0], payload: mark('early', enrollmentIds[0]) },
        { id: uuid(), enrollment: enrollmentIds[0], payload: mark(null, enrollmentIds[0]) },
        { id: uuid(), enrollment: enrollmentIds[1], payload: mark('excused', enrollmentIds[1]) },
        {
          id: uuid(),
          enrollment: null,
          payload: {
            kind: 'note',
            studentId,
            date,
            text: 'Private notebook note',
            tags: ['synthetic'],
          },
        },
      ]
      for (const { id, enrollment, payload } of events) {
        await pool.query(
          "INSERT INTO events (id,teacher_id,student_id,actor,client_time,payload,enrollment_id) VALUES ($1,$2,$3,'Synthetic teacher','2026-11-20T06:05:04.123456Z',$4,$5)",
          [id, teacherId, studentId, payload, enrollment],
        )
      }

      const signIn = async (phone: string, userId: string, endpoint = origin) => {
        const response = await fetch(`${endpoint}/api/auth/sign-in/phone`, {
          method: 'POST',
          headers: { origin, 'Content-Type': 'application/json' },
          body: JSON.stringify({ phone, password }),
        })
        expect(response.status).toBe(200)
        const cookie = response.headers
          .getSetCookie()
          .map((item) => item.split(';')[0])
          .join('; ')
        return { origin, cookie, 'x-user-id': userId }
      }
      const teacher = await signIn('+12025550181', users[0])
      const admin = await signIn('+12025550182', users[1])
      const student = await signIn('+12025550183', users[2])
      const same = async (path: string, status: number, init?: RequestInit) => {
        const [baseline, candidate] = await Promise.all(
          endpoints.map((url) => fetch(`${url}${path}`, init)),
        )
        expect(baseline.status).toBe(status)
        expect(candidate.status).toBe(status)
        for (const header of [
          'content-type',
          'access-control-allow-origin',
          'vary',
          'cache-control',
        ]) {
          expect(candidate.headers.get(header)).toBe(baseline.headers.get(header))
        }
        const expected: unknown = await baseline.json()
        expect(await candidate.json()).toEqual(expected)
        return expected
      }
      await same('/api/me', 401)
      await same('/api/auth/get-session', 200)
      await same('/api/auth/get-session', 200, { headers: teacher })
      const rustTeacher = await signIn('+12025550181', users[0], rust.origin)
      await same('/api/auth/get-session', 200, { headers: rustTeacher })
      await same('/api/me', 200, { headers: rustTeacher })
      await same('/api/device-accounts', 200, { headers: teacher })
      await same('/api/device-accounts', 200, { headers: rustTeacher })
      const jsonPost = (headers: typeof teacher, body: unknown): RequestInit => ({
        method: 'POST',
        headers: { ...headers, 'content-type': 'application/json' },
        body: JSON.stringify(body),
      })
      await same('/api/auth/update-user', 200, jsonPost(rustTeacher, { name: 'Native profile' }))
      await same('/api/auth/get-session', 200, { headers: rustTeacher })
      const changedPassword = 'updated-synthetic-password'
      const passwordChange = await fetch(
        `${rust.origin}/api/auth/change-password`,
        jsonPost(rustTeacher, { currentPassword: password, newPassword: changedPassword }),
      )
      expect(passwordChange.status).toBe(200)
      const loginWithNewPassword = await fetch(
        `${origin}/api/auth/sign-in/phone`,
        jsonPost(teacher, { phone: '+12025550181', password: changedPassword }),
      )
      expect(loginWithNewPassword.status).toBe(200)
      const signOut = await fetch(`${rust.origin}/api/auth/sign-out`, {
        method: 'POST',
        headers: rustTeacher,
      })
      expect(signOut.status).toBe(200)
      await same('/api/me', 401, { headers: rustTeacher })
      await same('/api/me', 200, { headers: teacher })
      await same('/api/diagnostics/telegram/conversations', 403, { headers: teacher })
      await same('/api/diagnostics/telegram/conversations?before=0', 400, { headers: admin })
      await same('/api/telegram/conversations', 401, { method: 'POST', body: '{}' })
      await same('/api/telegram/student', 401, { method: 'POST', body: '{}' })
      const conversationId = randomBytes(32).toString('hex')
      const turn = {
        trace,
        conversationId,
        question: 'Help with https://local.invalid?',
        reply: 'Use the help menu',
        delivery: 'sent',
        outcome: 'ok',
        promptVersion: 'wartaq-help-v1',
        model: 'synthetic',
        elapsedMs: 10,
      }
      const ingest = (body: unknown): RequestInit => ({
        method: 'POST',
        headers: {
          authorization: `Bearer ${telegramWorkerSecret}`,
          'content-type': 'application/json',
        },
        body: JSON.stringify(body),
      })
      expect(await same('/api/telegram/conversations', 200, ingest(turn))).toEqual({ ok: true })
      await same('/api/telegram/conversations', 409, ingest({ ...turn, reply: 'Different' }))
      await same('/api/diagnostics/telegram/conversations', 200, { headers: admin })
      expect(
        await same(`/api/diagnostics/telegram/turns?conversation=${conversationId}`, 200, {
          headers: admin,
        }),
      ).toEqual({
        items: [expect.objectContaining({ trace, question: 'Help with [redacted]' })],
        nextCursor: null,
      })
      for (const endpoint of endpoints) {
        const issued = await fetch(
          `${endpoint}/api/telegram/student`,
          ingest({ action: 'login-phone', phone: '+12025550183' }),
        )
        expect(issued.status).toBe(200)
        const { token, role } = z
          .object({
            token: z.string().regex(/^[A-Za-z0-9_-]{43}$/),
            role: z.literal('student'),
          })
          .parse(await issued.json())
        telegramGrants.push(
          `wartaq:telegram:${createHash('sha256').update(token).digest('hex')}:${users[2]}`,
        )
        expect(role).toBe('student')
        await same('/api/telegram/student', 200, ingest({ action: 'attendance', token }))
        const other = endpoint === origin ? rust.origin : origin
        const revoked = await fetch(
          `${other}/api/telegram/student`,
          ingest({ action: 'unlink', token }),
        )
        expect(revoked.status).toBe(200)
        await same('/api/telegram/student', 403, ingest({ action: 'attendance', token }))
      }
      await same('/api/me', 401, { headers: { ...teacher, 'x-user-id': users[1] } })
      await same('/api/me', 403, { headers: { ...admin, origin: 'https://foreign.invalid' } })
      await same('/api/me', 405, { headers: teacher, method: 'POST' })
      for (const headers of [teacher, admin, student]) await same('/api/me', 200, { headers })
      await same('/api/notebook', 200, { headers: teacher })
      await same('/api/notebook', 200, { headers: admin })
      await same('/api/notebook', 403, { headers: student })
      await same('/api/admin', 200, { headers: admin })
      await same('/api/admin', 403, { headers: teacher })
      await same('/api/student/attendance', 403, { headers: admin })
      expect(await same('/api/student/attendance', 200, { headers: student })).toEqual({
        student: { studentId, name: 'Synthetic learner', group: 'Roster level' },
        attendance: [
          { date, status: 'excused', enrollmentId: enrollmentIds[1], course: 'Álgebra' },
        ],
      })

      const post = (headers: typeof teacher, body: unknown): RequestInit => ({
        method: 'POST',
        headers: { ...headers, 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
      const event = <T>(payload: T) => ({
        id: uuid(),
        actor: 'Synthetic teacher',
        clientTime: '2026-11-21T12:00:00.543210Z',
        receivedAt: '',
        payload,
      })
      const unscoped = event({ kind: 'attendance', studentId, date: '2026-11-21', status: 'late' })
      await same('/api/events', 403, post(teacher, { events: [unscoped] }))
      await pool.query(
        'INSERT INTO events (id,teacher_id,student_id,actor,client_time,payload) VALUES ($1,$2,$3,$4,$5,$6)',
        [unscoped.id, teacherId, studentId, unscoped.actor, unscoped.clientTime, unscoped.payload],
      )
      await same('/api/events', 200, post(teacher, { events: [unscoped] }))
      const extra = event({
        kind: 'note',
        studentId,
        date,
        text: 'Rolled back note',
        tags: ['test'],
      })
      const different = { ...unscoped, payload: { ...unscoped.payload, status: 'absent' } }
      await same('/api/events', 409, post(teacher, { events: [extra, different] }))
      expect(
        (
          await pool.query('SELECT id FROM events WHERE id = ANY($1::uuid[])', [
            [unscoped.id, extra.id],
          ])
        ).rows,
      ).toEqual([{ id: unscoped.id }])

      const offline = event({ ...mark('present', enrollmentIds[0]), date: '2026-11-22' })
      const sync = (next: typeof offline) => ({
        accountId: users[0],
        records: [{ event: next, expectedEventId: null }],
      })
      expect(await same('/api/attendance/sync', 200, post(teacher, sync(offline)))).toEqual({
        outcomes: [
          { id: offline.id, kind: 'saved', current: { eventId: offline.id, status: 'present' } },
        ],
      })
      const stale = event({ ...mark('absent', enrollmentIds[0]), date: '2026-11-22' })
      expect(await same('/api/attendance/sync', 200, post(teacher, sync(stale)))).toEqual({
        outcomes: [
          { id: stale.id, kind: 'conflict', current: { eventId: offline.id, status: 'present' } },
        ],
      })
      expect(
        (
          await pool.query('SELECT id FROM events WHERE id = ANY($1::uuid[])', [
            [offline.id, stale.id],
          ])
        ).rows,
      ).toEqual([{ id: offline.id }])

      const gradePath = '/api/admin/enrollments/grades'
      const firstReceipt = uuid()
      receipts.push(firstReceipt)
      const grade = {
        kind: 'grade-edit',
        requestId: firstReceipt,
        id: enrollmentIds[0],
        grant: { id: adminId, expiresAt: null },
        fields: {
          notes: { next: 'Updated grade note', previous: 'Private grade note' },
          practical: { next: 1e-7, previous: 95 },
        },
      }
      expect(await same(gradePath, 200, post(admin, grade))).toEqual({ status: 'accepted' })
      expect(
        await same(
          gradePath,
          200,
          post(admin, { ...grade, fields: { practical: { previous: 95, next: 20 } } }),
        ),
      ).toEqual({ status: 'blocked' })
      expect(
        (
          await pool.query('SELECT practical::float8, notes FROM course_enrollments WHERE id=$1', [
            enrollmentIds[0],
          ])
        ).rows,
      ).toEqual([{ practical: 1e-7, notes: 'Updated grade note' }])
      await same(
        '/api/admin/courses/settings',
        200,
        post(admin, {
          id: courseIds[0],
          attendanceMinimum: 73,
          practicalMinimum: 82,
          theoryMinimum: 79,
        }),
      )
      await same(
        '/api/admin/enrollments/handbook',
        200,
        post(admin, { id: enrollmentIds[0], handbookReceived: true }),
      )
      expect(
        (
          await pool.query('SELECT handbook_received FROM course_enrollments WHERE id=$1', [
            enrollmentIds[0],
          ])
        ).rows,
      ).toEqual([{ handbook_received: true }])
      const membership = { id: enrollmentIds[0], courseId: courseIds[0], studentId, active: false }
      await same('/api/admin/enrollments/membership', 200, post(admin, membership))
      expect(
        (
          await pool.query('SELECT active,handbook_received FROM course_enrollments WHERE id=$1', [
            enrollmentIds[0],
          ])
        ).rows,
      ).toEqual([{ active: false, handbook_received: true }])
      await same(
        '/api/admin/enrollments/membership',
        200,
        post(admin, { ...membership, active: true }),
      )
      await same(
        '/api/admin/enrollments/assignment',
        200,
        post(admin, {
          kind: 'edit',
          id: enrollmentIds[0],
          courseId: courseIds[0],
          teacherId,
          level: 'Assigned level',
        }),
      )
      await same('/api/notebook', 200, { headers: admin })
      const expectedProfile = {
        firstName: 'Synthetic learner',
        lastName: '',
        group: 'Roster level',
        whatsapp: null,
        location: null,
        birthYear: null,
      }
      const profileUpdate = post(teacher, {
        ...expectedProfile,
        studentId,
        expected: expectedProfile,
        lastName: 'Updated',
        birthYear: 2005,
      })
      const profileSaved = await fetch(`${rust.origin}/api/student/update`, profileUpdate)
      expect(profileSaved.status).toBe(200)
      expect(await profileSaved.json()).toEqual({ saved: true })
      expect(
        (await pool.query('SELECT name, birth_year FROM students WHERE id=$1', [studentId])).rows,
      ).toEqual([{ name: 'Synthetic learner Updated', birth_year: 2005 }])
      await same('/api/student/update', 409, profileUpdate)
      await same('/api/student/update', 403, post(student, {}))
      const note = event({
        kind: 'note',
        studentId,
        date,
        text: 'Local offline note',
        tags: ['test'],
      })
      const studentSync = (operation: unknown) => post(teacher, { accountId: users[0], operation })
      await same('/api/student/sync', 200, studentSync({ kind: 'note', event: note }))
      await same(
        '/api/student/sync',
        200,
        studentSync({
          kind: 'contact',
          id: uuid(),
          studentId,
          clientTime: note.clientTime,
          expectedWhatsapp: null,
          whatsapp: ' +12025550199 ',
        }),
      )
      await same(
        '/api/student/sync',
        200,
        studentSync({
          kind: 'profile-v1',
          id: uuid(),
          studentId,
          clientTime: note.clientTime,
          expected: { location: null },
          changes: { location: ' Local classroom ' },
        }),
      )
      expect(
        (await pool.query('SELECT whatsapp, location FROM students WHERE id=$1', [studentId])).rows,
      ).toEqual([{ whatsapp: '+12025550199', location: 'Local classroom' }])
      await same('/api/student/attendance', 200, { headers: student })
      const journeyId = uuid()
      const diagnosticsId = uuid()
      await same(
        '/api/diagnostics',
        200,
        post(teacher, {
          events: [{ id: diagnosticsId, journeyId, step: 1, kind: 'screen', screen: 'attendance' }],
        }),
      )
      expect(
        (
          await pool.query('SELECT event_id FROM website_diagnostics WHERE event_id=$1', [
            diagnosticsId,
          ])
        ).rows,
      ).toEqual([{ event_id: diagnosticsId }])
      await same(`/api/diagnostics/journeys?actor=${users[0]}`, 200, { headers: admin })
      await same(`/api/diagnostics/events?actor=${users[0]}&journey=${journeyId}`, 200, {
        headers: admin,
      })
      const frame = {
        kind: 'click',
        step: 1,
        timeMs: 200,
        screen: 'attendance',
        width: 320,
        height: 600,
        theme: 'light',
        language: 'en',
        click: { x: 90, y: 180 },
        boxes: [{ x: 20, y: 160, w: 150, h: 40, kind: 'control', label: 'attendance' }],
      }
      await same('/api/diagnostics/replay', 200, post(teacher, { journeyId, frames: [frame] }))
      const replay = `/api/diagnostics/replay?actor=${users[0]}&journey=${journeyId}`
      await same(replay, 403, { headers: teacher })
      expect(await same(replay, 200, { headers: admin })).toEqual({
        items: [frame],
        nextAfter: null,
      })
      await pool.query(
        "INSERT INTO events (id,teacher_id,student_id,actor,client_time,payload) VALUES ($1,$2,$3,'Synthetic teacher',now(),$4)",
        [uuid(), teacherId, studentId, { kind: 'unknown' }],
      )
      await same('/api/student/attendance', 400, { headers: student })
      await same('/api/notebook', 400, { headers: teacher })
      await pool.query(
        'UPDATE app_accounts SET invitation_request_id = $2, invitation_requires_setup = true WHERE auth_user_id = $1',
        [users[2], uuid()],
      )
      await same('/api/student/attendance', 403, { headers: student })
      await same('/api/me/invitation-profile', 400, jsonPost(student, { newPassword: password }))
      const otherStudentSession = await signIn('+12025550183', users[2], rust.origin)
      const invitation = await fetch(
        `${rust.origin}/api/me/invitation-profile`,
        jsonPost(student, { newPassword: changedPassword }),
      )
      expect(invitation.status).toBe(200)
      expect(await invitation.json()).toEqual({ saved: true, sessionRevoked: false })
      await same('/api/me', 200, { headers: student })
      await same('/api/me', 401, { headers: otherStudentSession })
      await pool.query(
        "UPDATE app_accounts SET expires_at = '2000-01-01T00:00:00Z' WHERE auth_user_id = $1",
        [users[0]],
      )
      await same('/api/me', 403, { headers: teacher })
      await pool.query('DELETE FROM neon_auth.session WHERE "userId" = $1', [users[2]])
      await same('/api/me', 401, { headers: student })
    } finally {
      bun?.child.kill()
      rust?.child.kill()
      await Promise.all([bun?.child.exited, rust?.exited])
      await pool.query('DELETE FROM events WHERE student_id = $1', [studentId])
      await pool.query('DELETE FROM website_diagnostics WHERE actor_id = ANY($1::text[])', [users])
      await pool.query('DELETE FROM telegram_conversation_turns WHERE trace = $1', [trace])
      await pool.query('DELETE FROM neon_auth.verification WHERE identifier = ANY($1::text[])', [
        telegramGrants,
      ])
      await pool.query('DELETE FROM website_session_replay WHERE actor_id = ANY($1::text[])', [
        users,
      ])
      await pool.query('DELETE FROM grade_sync_receipts WHERE request_id = ANY($1::uuid[])', [
        receipts,
      ])
      await pool.query('DELETE FROM app_accounts WHERE auth_user_id = ANY($1::text[])', [users])
      await pool.query('DELETE FROM course_enrollments WHERE id = ANY($1::uuid[])', [enrollmentIds])
      await pool.query('DELETE FROM courses WHERE id = ANY($1::uuid[])', [courseIds])
      await pool.query('DELETE FROM students WHERE id = ANY($1::uuid[])', [
        [studentId, historicalId],
      ])
      await pool.query('DELETE FROM teachers WHERE id = ANY($1::uuid[])', [[teacherId, adminId]])
      await pool.query('DELETE FROM neon_auth."user" WHERE id = ANY($1::uuid[])', [users])
      await pool.end()
    }
  },
  30_000,
)
