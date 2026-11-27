import { expect, test } from 'bun:test'
import { Pool } from 'pg'

const databaseUrl = process.env.TEST_RAILWAY_DATABASE_URL
const integration = databaseUrl ? test : test.skip

integration('record versions reject stale edits regardless of arrival order', async () => {
  if (!databaseUrl) throw new Error('Disposable database required')
  const pool = new Pool({ connectionString: databaseUrl })
  const db = await pool.connect()
  try {
    await db.query('BEGIN')
    const studentId = crypto.randomUUID()
    const courseId = crypto.randomUUID()
    const ids = [crypto.randomUUID(), crypto.randomUUID()]
    await db.query("INSERT INTO students (id, name, group_name) VALUES ($1, 'Synthetic', '')", [
      studentId,
    ])
    await db.query("INSERT INTO courses (id, name) VALUES ($1, 'Synthetic')", [courseId])
    await db.query(
      'INSERT INTO course_enrollments (id, course_id, student_id, level) VALUES ($1, $2, $3, $4)',
      [ids[0], courseId, studentId, 'base'],
    )
    // A course/student pair is unique, so create the second record for another course.
    const otherCourse = crypto.randomUUID()
    await db.query("INSERT INTO courses (id, name) VALUES ($1, 'Synthetic 2')", [otherCourse])
    await db.query(
      'INSERT INTO course_enrollments (id, course_id, student_id, level) VALUES ($1, $2, $3, $4)',
      [ids[1], otherCourse, studentId, 'base'],
    )
    const older = new Date(Date.now() + 10_000).toISOString()
    const newer = new Date(Date.now() + 20_000).toISOString()
    async function save(id: string, time: string, level: string, editId = crypto.randomUUID()) {
      await db.query(
        "SELECT set_config('wartaq.edit_time', $1, true), set_config('wartaq.edit_id', $2, true)",
        [time, editId],
      )
      await db.query('UPDATE course_enrollments SET level = $2 WHERE id = $1', [id, level])
    }
    await save(ids[0], older, 'old')
    const winningId = 'ffffffff-ffff-4fff-bfff-ffffffffffff'
    await save(ids[0], newer, 'new', winningId)
    await save(ids[1], newer, 'new', winningId)
    async function rejects(edit: () => Promise<void>) {
      await db.query('SAVEPOINT rejected_edit')
      let errorCode: unknown
      try {
        await edit()
      } catch (error) {
        errorCode = error instanceof Error && 'code' in error ? error.code : undefined
      } finally {
        await db.query('ROLLBACK TO SAVEPOINT rejected_edit')
      }
      expect(errorCode).toBe('W0001')
    }
    await rejects(() => save(ids[1], older, 'old'))
    await rejects(() => save(ids[1], newer, 'lower UUID', '00000000-0000-4000-8000-000000000000'))
    await rejects(() => save(ids[1], new Date(Date.now() + 600_000).toISOString(), 'fast clock'))
    const result = await db.query(
      'SELECT level FROM course_enrollments WHERE id = ANY($1::uuid[])',
      [ids],
    )
    expect(result.rows.map((row) => row.level)).toEqual(['new', 'new'])
  } finally {
    await db.query('ROLLBACK')
    db.release()
    await pool.end()
  }
})
