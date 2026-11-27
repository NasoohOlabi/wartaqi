import { expect, test, type Page } from '@playwright/test'

const userId = 'synthetic-cached-student'
const studentId = '439a3c8c-50d4-48cf-9b47-9ecd2b6189aa'
const otherStudentId = 'c68e0569-dd55-4071-b12f-ed6cb566fae4'
const date = '2026-11-18'

async function mockSession(
  page: Page,
  userId: string,
  connected: () => boolean,
  beforeReply: () => Promise<void> = () => Promise.resolve(),
) {
  await page.route('**/api/auth/get-session*', async (route) => {
    if (!connected()) return route.abort('internetdisconnected')
    await beforeReply()
    const now = new Date().toISOString()
    await route.fulfill({
      json: {
        user: {
          id: userId,
          name: 'Cached Student',
          email: 'student@example.test',
          emailVerified: true,
          createdAt: now,
          updatedAt: now,
        },
        session: {
          id: 'synthetic-session',
          userId,
          token: 'synthetic-token',
          expiresAt: new Date(Date.now() + 60_000).toISOString(),
          createdAt: now,
          updatedAt: now,
        },
      },
    })
  })
}

function storedReplicas(page: Page) {
  return page.evaluate(async () => {
    const databases = await indexedDB.databases()
    if (!databases.some(({ name }) => name === 'wartaq-role-replicas')) return ''
    const stored = await new Promise<unknown>((resolve, reject) => {
      const opening = indexedDB.open('wartaq-role-replicas', 1)
      opening.onerror = () => reject(opening.error)
      opening.onsuccess = () => {
        const db = opening.result
        const tx = db.transaction('replicas')
        const read = tx.objectStore('replicas').getAll()
        tx.oncomplete = () => {
          db.close()
          resolve(read.result)
        }
        tx.onabort = () => {
          db.close()
          reject(tx.error)
        }
      }
    })
    return JSON.stringify(stored)
  })
}

test('warm attendance is scoped to the last grant and disappears on confirmed withdrawal', async ({
  context,
  page,
}) => {
  let assigned = studentId
  let active = true
  let connected = true
  let hold = Promise.resolve()
  let release = () => {}
  let finishingAttendance = Promise.resolve()
  let finishAttendance = () => {}
  await mockSession(
    page,
    userId,
    () => connected,
    () => hold,
  )
  await page.route('**/api/me', async (route) => {
    if (!connected) return route.abort('internetdisconnected')
    if (!active) return route.fulfill({ status: 403, json: { error: 'forbidden' } })
    await route.fulfill({
      json: {
        account: {
          role: 'student',
          id: assigned,
          name: assigned === studentId ? 'Cached Student' : 'New Student',
          canRead: true,
          canWrite: false,
          expiresAt: null,
        },
      },
    })
  })
  await page.route('**/api/student/attendance', async (route) => {
    if (!connected) return route.abort('internetdisconnected')
    const id = assigned
    await finishingAttendance
    await route.fulfill({
      json: {
        student: {
          studentId: id,
          name: id === studentId ? 'Cached Student' : 'New Student',
          group: 'Group 2',
          notes: ['synthetic-private-note'],
        },
        attendance: [{ date, status: 'present' }],
        invitationMessage: 'synthetic-private-invitation',
      },
    })
  })
  await page.route('**/api/diagnostics*', (route) => route.fulfill({ json: { ok: true } }))
  await page.route('**/api/device-accounts', (route) => route.fulfill({ json: { accounts: [] } }))

  await page.goto('/?page=app')
  await expect(page.getByRole('cell', { name: date })).toBeVisible()
  await expect
    .poll(() => storedReplicas(page))
    .toBe(
      JSON.stringify([
        {
          student: { studentId, name: 'Cached Student', group: 'Group 2' },
          attendance: [{ date, status: 'present' }],
        },
      ]),
    )

  await page.evaluate(() => navigator.serviceWorker.ready)
  await page.reload()
  await expect(page.getByRole('cell', { name: date })).toBeVisible()
  connected = false
  await context.setOffline(true)
  await page.reload()
  await expect(page.getByText('Offline', { exact: true })).toBeVisible()
  await expect(page.getByRole('cell', { name: date })).toBeVisible()

  hold = new Promise<void>((resolve) => (release = resolve))
  connected = true
  await context.setOffline(false)
  await page.reload()
  await expect(page.getByRole('cell', { name: date })).toBeVisible()
  await expect(page.getByText('Offline', { exact: true })).toHaveCount(0)

  active = false
  finishingAttendance = new Promise<void>((resolve) => (finishAttendance = resolve))
  release()
  await expect(page.getByRole('alert')).toContainText('no longer has access')
  await expect(page.getByRole('cell', { name: date })).toHaveCount(0)

  assigned = otherStudentId
  active = true
  await page.reload()
  await expect(page.getByRole('heading', { name: 'My attendance' })).toBeVisible()
  await expect(page.getByText('Cached Student', { exact: true })).toHaveCount(0)
  await expect(page.getByRole('cell', { name: date })).toHaveCount(0)
  finishAttendance()
  await expect(page.getByText('New Student', { exact: true })).toBeVisible()
})

test('administrator opens a prewarmed board during a stalled refresh and preserves a draft', async ({
  page,
}) => {
  const adminId = 'f2aa3abe-de37-4bc0-8e81-279ca99c9bbd'
  const teacherId = 'ba055442-3e70-4aed-879a-8e05ebbbec4f'
  const account = {
    role: 'administrator',
    id: adminId,
    name: 'Administrator',
    canRead: true,
    canWrite: true,
    expiresAt: null,
  }
  let boardRead = Promise.resolve()
  let releaseBoard = () => {}
  let profileName = 'Teacher Name'
  await mockSession(page, adminId, () => true)
  await page.route('**/api/me', (route) => route.fulfill({ json: { account } }))
  await page.route('**/api/notebook', (route) =>
    route.fulfill({ json: { teacher: account, students: [], events: [] } }),
  )
  await page.route('**/api/diagnostics*', (route) => route.fulfill({ json: { ok: true } }))
  await page.route('**/api/admin', async (route) => {
    await boardRead
    await route.fulfill({
      json: {
        accounts: [
          {
            authUserId: teacherId,
            role: 'teacher',
            name: profileName,
            active: true,
            expiresAt: null,
            studentId: null,
            teacherId,
          },
        ],
        teachers: [],
        students: [],
      },
    })
  })

  const prewarm = page.waitForResponse((response) => response.url().endsWith('/api/admin'))
  await page.goto('/?page=app')
  await prewarm
  await expect.poll(() => storedReplicas(page)).toContain('Teacher Name')

  boardRead = new Promise<void>((resolve) => (releaseBoard = resolve))
  await page.goto(`/?page=app&view=settings&account=${teacherId}`)
  await expect(page.getByText('Teacher Name', { exact: true })).toBeVisible()
  const name = page.getByLabel('Name', { exact: true })
  await name.fill('Unsaved edit')
  profileName = 'Updated name from another device'
  releaseBoard()
  await expect(page.getByText(profileName, { exact: true })).toBeVisible()
  await expect(name).toHaveValue('Unsaved edit')
})
