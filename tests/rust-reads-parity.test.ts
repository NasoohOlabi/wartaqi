import { expect, test } from 'bun:test'
import { Pool } from 'pg'
import { createIdentity } from './integration-identity'
import { startIntegrationServer } from './integration-server'
import { startRustBackend } from './rust-backend-server'

const databaseUrl = process.env.TEST_RAILWAY_DATABASE_URL
const binary = process.env.TEST_RUST_SYNC_BINARY
const parity = databaseUrl && binary ? test : test.skip

parity(
  'Rust and Bun return identical role-scoped reads for the same sessions',
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
    const password = 'synthetic-parity-password'
    try {
      bun = await startIntegrationServer(databaseUrl)
      const { origin } = bun
      rust = await startRustBackend(binary, databaseUrl, origin)
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

      const signIn = async (phone: string, userId: string) => {
        const response = await fetch(`${origin}/api/auth/sign-in/phone`, {
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
      await pool.query(
        "INSERT INTO events (id,teacher_id,student_id,actor,client_time,payload) VALUES ($1,$2,$3,'Synthetic teacher',now(),$4)",
        [uuid(), teacherId, studentId, { kind: 'unknown' }],
      )
      await same('/api/student/attendance', 400, { headers: student })
      await same('/api/notebook', 400, { headers: teacher })
      await pool.query(
        "UPDATE app_accounts SET expires_at = '2000-01-01T00:00:00Z' WHERE auth_user_id = $1",
        [users[0]],
      )
      await same('/api/me', 403, { headers: teacher })
    } finally {
      bun?.child.kill()
      rust?.child.kill()
      await Promise.all([bun?.child.exited, rust?.child.exited])
      await pool.query('DELETE FROM events WHERE student_id = $1', [studentId])
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
