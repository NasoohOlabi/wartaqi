import { randomBytes, randomInt } from 'node:crypto'
import { test, expect, type Browser } from '@playwright/test'
import { Pool } from 'pg'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { z } from 'zod'
import { createIdentity } from '../tests/integration-identity'

// Requires a migrated disposable database, a production build and Playwright Chromium.
const databaseUrl = process.env.TEST_RAILWAY_DATABASE_URL
test.use({ trace: 'off' })
test('offline teacher edits reach a second signed-in student over live sync', async ({
  browser,
}, testInfo) => {
  test.skip(
    !databaseUrl || testInfo.project.name !== 'chromium',
    'Runs once against a disposable migrated database',
  )
  test.setTimeout(45000)
  if (!databaseUrl) throw new Error('Set TEST_RAILWAY_DATABASE_URL to a disposable database')
  const db = new Pool({ connectionString: databaseUrl })
  const teacherId = crypto.randomUUID()
  const studentId = crypto.randomUUID()
  const courseId = crypto.randomUUID()
  const enrollmentId = crypto.randomUUID()
  const password = randomBytes(24).toString('hex')
  const phonePrefix = '+1202' + randomInt(100000, 999999)
  const teacherPhone = phonePrefix + '1'
  const studentPhone = phonePrefix + '2'
  const identities: string[] = []
  const server = spawn('bun', ['tests/integration-server.ts'], {
    env: {
      ...process.env,
      DATABASE_URL: databaseUrl,
      BETTER_AUTH_SECRET: randomBytes(32).toString('hex'),
    },
    stdio: ['ignore', 'ignore', 'inherit', 'ipc'],
    serialization: 'json',
  })
  try {
    const [address] = await Promise.race([
      once(server, 'message', { signal: AbortSignal.timeout(12000) }),
      once(server, 'exit').then(() => {
        throw new Error('Fixture server exited during startup')
      }),
    ])
    const origin = z
      .string()
      .regex(/^http:\/\/127\.0\.0\.1:\d+$/)
      .parse(address)
    identities.push(await createIdentity(db, 'Sync Teacher', password))
    identities.push(await createIdentity(db, 'Sync Student', password))
    await db.query(
      "INSERT INTO teachers (id, auth_user_id, name, access_mode) VALUES ($1,$2,$3,'read_write')",
      [teacherId, identities[0], 'Sync Teacher'],
    )
    await db.query('INSERT INTO students (id, teacher_id, name) VALUES ($1,$2,$3)', [
      studentId,
      teacherId,
      'Sync Student',
    ])
    await db.query('INSERT INTO courses (id, name) VALUES ($1,$2)', [courseId, 'Sync Course'])
    await db.query('INSERT INTO course_enrollments (id, student_id, course_id) VALUES ($1,$2,$3)', [
      enrollmentId,
      studentId,
      courseId,
    ])
    await db.query(
      `INSERT INTO app_accounts (auth_user_id, role, teacher_id, student_id, verified_phone)
    VALUES ($1,'teacher',$2,NULL,$3), ($4,'student',NULL,$5,$6)`,
      [identities[0], teacherId, teacherPhone, identities[1], studentId, studentPhone],
    )

    async function device(chromiumBrowser: Browser, phone: string) {
      const context = await chromiumBrowser.newContext({ baseURL: origin })
      context.setDefaultTimeout(10000)
      const login = await context.request.post('/api/auth/sign-in/phone', {
        headers: { origin },
        data: { phone, password },
      })
      if (!login.ok()) throw new Error(`Fixture sign-in failed with ${login.status()}`)
      await context.addInitScript(() => localStorage.setItem('halaqa-language-preference', 'en'))
      return context
    }
    const [staff, learner] = await Promise.all([
      device(browser, teacherPhone),
      device(browser, studentPhone),
    ])
    const teacher = await staff.newPage()
    const student = await learner.newPage()
    const attendanceUrl = '/?page=app&tab=attendance&course=' + courseId + '&date=2026-11-18'
    await Promise.all([teacher.goto(attendanceUrl), student.goto('/?page=app')])
    await expect(student.getByRole('heading', { name: 'My attendance' })).toBeVisible()
    const mark = teacher
      .getByLabel('Attendance for Sync Student')
      .getByRole('button', { name: 'present', exact: true })
    await expect(mark).toBeEnabled()
    await expect
      .poll(() =>
        teacher.evaluate(
          async () => (await navigator.serviceWorker.getRegistration())?.active?.state,
        ),
      )
      .toBe('activated')
    await expect
      .poll(() => teacher.evaluate(() => Boolean(localStorage.getItem('halaqa-offline-access'))))
      .toBe(true)
    // A freshly installed worker controls the next navigation.
    await teacher.reload()
    await expect.poll(() => teacher.evaluate(() => !!navigator.serviceWorker.controller)).toBe(true)
    await staff.setOffline(true)
    await teacher.reload()
    await mark.click()
    await expect(teacher.getByText('Saved on this device. Waiting to sync.')).toBeVisible()
    await teacher.reload()
    await expect(mark).toHaveAttribute('aria-pressed', 'true')
    await staff.setOffline(false)
    await expect(student.getByRole('cell', { name: 'present', exact: true })).toBeVisible({
      timeout: 12000,
    })
    const result = await db.query(
      "SELECT count(*)::int AS n FROM events WHERE student_id=$1 AND payload->>'kind'='attendance'",
      [studentId],
    )
    expect(result.rows[0].n).toBe(1)
    // Hold a revalidation request across a navigation. Cached student data must paint first.
    const held = Promise.withResolvers<void>()
    await student.route('**/api/student/attendance', async (route) => {
      await held.promise
      await route.continue().catch(() => {})
    })
    try {
      await student.reload()
      await expect(student.getByRole('cell', { name: 'present', exact: true })).toBeVisible()
    } finally {
      held.resolve()
    }
    await student.unroute('**/api/student/attendance')
    await staff.setOffline(true)
    await teacher.reload()
    await expect(mark).toHaveAttribute('aria-pressed', 'true')
    await mark.click()
    await expect(teacher.getByText('Saved on this device. Waiting to sync.')).toBeVisible()
    await teacher.reload()
    await expect(mark).toHaveAttribute('aria-pressed', 'false')
    await staff.setOffline(false)
    await expect(student.getByRole('cell', { name: 'present', exact: true })).toHaveCount(0, {
      timeout: 12000,
    })
    const cleared = await db.query(
      "SELECT payload->'status' AS status FROM events WHERE student_id=$1 AND payload->>'kind'='attendance'",
      [studentId],
    )
    expect(cleared.rows).toEqual(expect.arrayContaining([{ status: 'present' }, { status: null }]))
    expect(cleared.rowCount).toBe(2)
    await teacher.reload()
    await expect(mark).toHaveAttribute('aria-pressed', 'false')
    await mark.click()
    await expect(student.getByRole('cell', { name: 'present', exact: true })).toBeVisible({
      timeout: 12000,
    })
    await Promise.all([staff.close(), learner.close()])
  } finally {
    server.kill()
    await db.query('DELETE FROM events WHERE student_id=$1', [studentId])
    await db.query('DELETE FROM app_accounts WHERE auth_user_id=ANY($1::text[])', [identities])
    await db.query('DELETE FROM course_enrollments WHERE id=$1', [enrollmentId])
    await db.query('DELETE FROM courses WHERE id=$1', [courseId])
    await db.query('DELETE FROM students WHERE id=$1', [studentId])
    await db.query('DELETE FROM teachers WHERE id=$1', [teacherId])
    await db.query('DELETE FROM neon_auth."user" WHERE id=ANY($1::uuid[])', [identities])
    await db.end()
  }
})
