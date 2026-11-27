import { expect, test } from 'bun:test'
import { Pool } from 'pg'
import { createHandler } from '../functions/notebook-service'
import { gradeOperationSchema, gradeOutcomeSchema, gradePath } from '../shared/gradeSync'

const databaseUrl = process.env.TEST_RAILWAY_DATABASE_URL
const integration = databaseUrl ? test : test.skip

integration(
  'grade edits merge unrelated fields and deduplicate lost acknowledgements after later edits',
  async () => {
    if (!databaseUrl) throw new Error('Disposable database required')
    const pool = new Pool({ connectionString: databaseUrl })
    const adminId = crypto.randomUUID()
    const admin = `sync-admin-${adminId}`
    const otherId = crypto.randomUUID()
    const other = `sync-other-${otherId}`
    const course = crypto.randomUUID()
    const student = crypto.randomUUID()
    const enrollment = crypto.randomUUID()
    const receiptIds: string[] = []
    const origin = 'https://grade-sync.example'
    const handler = createHandler(pool, async (token) => token, [origin])

    function operation(fields: unknown) {
      const requestId = crypto.randomUUID()
      receiptIds.push(requestId)
      return gradeOperationSchema.parse({
        kind: 'grade-edit',
        requestId,
        id: enrollment,
        grant: { id: adminId, expiresAt: null },
        fields,
      })
    }
    function post(body: unknown, subject = admin) {
      return handler(
        new Request(origin + gradePath, {
          method: 'POST',
          headers: {
            origin,
            authorization: `Bearer ${subject}`,
            'Content-Type': 'application/json',
          },
          body: JSON.stringify(body),
        }),
      )
    }
    async function outcome(body: unknown, subject = admin) {
      const response = await post(body, subject)
      expect(response.status).toBe(200)
      return gradeOutcomeSchema.parse(await response.json())
    }
    try {
      for (const [id, subject] of [
        [adminId, admin],
        [otherId, other],
      ]) {
        await pool.query(
          "INSERT INTO teachers (id, auth_user_id, name, access_mode) VALUES ($1, $2, 'Synthetic sync admin', 'read_write')",
          [id, subject],
        )
        await pool.query(
          "INSERT INTO app_accounts (auth_user_id, role, teacher_id) VALUES ($1, 'administrator', $2)",
          [subject, id],
        )
      }
      await pool.query("INSERT INTO courses (id, name) VALUES ($1, 'Synthetic course')", [course])
      await pool.query(
        "INSERT INTO students (id, teacher_id, name) VALUES ($1, $2, 'Synthetic student')",
        [student, adminId],
      )
      await pool.query(
        "INSERT INTO course_enrollments (id, course_id, student_id, level, practical, theory, notes) VALUES ($1, $2, $3, '2', 40, 50, 'before')",
        [enrollment, course, student],
      )

      const first = operation({ practical: { previous: 40, next: 60 } })
      const unrelated = operation({ theory: { previous: 50, next: 70 } })
      expect(await Promise.all([outcome(first), outcome(unrelated)])).toEqual([
        { status: 'accepted' },
        { status: 'accepted' },
      ])
      const subsequent = operation({
        practical: { previous: 60, next: 80 },
        notes: { previous: 'before', next: 'later' },
      })
      expect(await outcome(subsequent)).toEqual({ status: 'accepted' })
      expect(await outcome(first)).toEqual({ status: 'accepted' })
      expect(
        await outcome({ ...first, fields: { practical: { previous: 40, next: 99 } } }),
      ).toEqual({ status: 'blocked' })
      expect(await outcome({ ...first, grant: { id: otherId, expiresAt: null } }, other)).toEqual({
        status: 'blocked',
      })
      expect(
        await outcome(
          operation({
            practical: { previous: 40, next: 90 },
            notes: { previous: 'later', next: 'unsaved' },
          }),
        ),
      ).toEqual({ status: 'conflict', current: { practical: 80, theory: 70, notes: 'later' } })
      expect(await outcome(operation({ practical: { previous: 40, next: 80 } }))).toEqual({
        status: 'accepted',
      })
      const saved = await pool.query(
        'SELECT practical::float8, theory::float8, notes, level FROM course_enrollments WHERE id = $1',
        [enrollment],
      )
      expect(saved.rows).toEqual([{ practical: 80, theory: 70, notes: 'later', level: '2' }])
      await pool.query('UPDATE courses SET open = false WHERE id = $1', [course])
      expect((await post({ id: enrollment, practical: 81, theory: 70 })).status).toBe(200)
      expect(await outcome(operation({ practical: { previous: 81, next: 82 } }))).toEqual({
        status: 'accepted',
      })
      const closedGrade = await pool.query(
        'SELECT ce.practical::float8, c.open FROM course_enrollments ce JOIN courses c ON c.id = ce.course_id WHERE ce.id = $1',
        [enrollment],
      )
      expect(closedGrade.rows).toEqual([{ practical: 82, open: false }])
      const expiresAt = new Date(Date.now() + 86400000).toISOString()
      await pool.query('UPDATE app_accounts SET expires_at = $2 WHERE auth_user_id = $1', [
        admin,
        expiresAt,
      ])
      expect(await outcome(first)).toEqual({ status: 'blocked' })
      const recovery = {
        kind: 'recover-grade-edit',
        operation: first,
        grant: { id: adminId, expiresAt },
      }
      expect(await outcome(recovery)).toEqual({ status: 'accepted' })
      expect(
        await outcome({ ...recovery, grant: { id: otherId, expiresAt: null } }, other),
      ).toEqual({ status: 'blocked' })
      expect(
        (
          await pool.query('SELECT practical::float8 FROM course_enrollments WHERE id = $1', [
            enrollment,
          ])
        ).rows,
      ).toEqual([{ practical: 82 }])
      expect(
        await outcome({
          ...recovery,
          operation: operation({ practical: { previous: 82, next: 85 } }),
        }),
      ).toEqual({ status: 'accepted' })
      await pool.query("UPDATE app_accounts SET role = 'teacher' WHERE auth_user_id = $1", [admin])
      expect((await post(first)).status).toBe(403)
      expect((await post(recovery)).status).toBe(403)
    } finally {
      await pool.query('DELETE FROM grade_sync_receipts WHERE request_id = ANY($1::uuid[])', [
        receiptIds,
      ])
      await pool.query('DELETE FROM course_enrollments WHERE id = $1', [enrollment])
      await pool.query('DELETE FROM courses WHERE id = $1', [course])
      await pool.query('DELETE FROM students WHERE id = $1', [student])
      await pool.query('DELETE FROM app_accounts WHERE auth_user_id = ANY($1::text[])', [
        [admin, other],
      ])
      await pool.query('DELETE FROM teachers WHERE id = ANY($1::uuid[])', [[adminId, otherId]])
      await pool.end()
    }
  },
  20_000,
)
