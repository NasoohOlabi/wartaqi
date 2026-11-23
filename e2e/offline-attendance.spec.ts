import { expect, test } from '@playwright/test'
import {
  offlineAttendanceRecordSchema,
  offlineAttendanceSyncSchema,
} from '../shared/offlineAttendance'
import { z } from 'zod'
import { offlineStudentSyncSchema } from '../shared/offlineStudent'
import type { LedgerEvent } from '../shared/ledger'

const accountId = 'offline-teacher'
const courseId = '40000000-0000-4000-8000-000000000001'
const studentIds = ['20000000-0000-4000-8000-000000000001', '20000000-0000-4000-8000-000000000002']
const names = ['Offline Student One', 'Offline Student Two']
const unenrolledId = '20000000-0000-4000-8000-000000000003'
const unenrolledName = 'Offline Student Without Enrollment'
const oldPhone = '+15550101010'
const otherPhone = '+15550101011'
const updatedPhone = '+15550101012'
const conflictingId = '30000000-0000-4000-8000-000000000001'
const date = '2026-11-18'
const courseUrl = `/?page=app&tab=attendance&course=${courseId}&date=${date}`

test('the full roster, attendance, notes and phone edits survive a cold offline restart and reconcile', async ({
  context,
  page,
}, testInfo) => {
  test.setTimeout(60_000)
  const teacher = {
    role: 'teacher',
    id: '10000000-0000-4000-8000-000000000001',
    name: 'Offline Teacher',
    canRead: true,
    canWrite: true,
    expiresAt: null,
  }
  const now = new Date().toISOString()
  const snapshot = {
    teacher,
    students: [...studentIds, unenrolledId].map((studentId, index) => ({
      kind: 'student',
      studentId,
      name: names[index] ?? unenrolledName,
      group: '',
      active: true,
      whatsapp: studentId === unenrolledId ? oldPhone : null,
    })),
    courses: [{ id: courseId, name: 'Offline Test Course', open: true }],
    enrollments: studentIds.map((studentId) => ({
      id: studentId,
      courseId,
      studentId,
      level: '',
      practical: null,
      theory: null,
      active: true,
    })),
  }
  const session = {
    user: {
      id: accountId,
      name: teacher.name,
      email: 'offline@example.test',
      emailVerified: true,
      createdAt: now,
      updatedAt: now,
    },
    session: {
      id: 'synthetic-session',
      userId: accountId,
      token: 'opaque-session',
      expiresAt: new Date(Date.now() + 3600000).toISOString(),
      createdAt: now,
      updatedAt: now,
    },
  }
  const attempts: string[] = []
  const accepted = new Map<string, z.infer<typeof offlineAttendanceRecordSchema>['event']>()
  const notes = new Map<string, LedgerEvent>()
  const contactReceipts = new Set<string>()
  const studentAttempts: string[] = []
  let serverPhone = oldPhone
  let lostReply = false
  let lostNoteReply = false
  let rejectFirstStudent = true
  let networkAvailable = true
  await context.route('**/api/**', async (route) => {
    if (!networkAvailable) return route.abort('internetdisconnected')
    const path = new URL(route.request().url()).pathname
    if (path === '/api/auth/get-session') return route.fulfill({ json: session })
    if (path === '/api/device-accounts') return route.fulfill({ json: { accounts: [] } })
    expect(route.request().headers()['x-user-id']).toBe(accountId)
    if (path === '/api/diagnostics') return route.fulfill({ json: { ok: true } })
    if (path === '/api/me') return route.fulfill({ json: { account: teacher, teacher } })
    if (path === '/api/notebook')
      return route.fulfill({
        json: {
          ...snapshot,
          students: snapshot.students.map((student) =>
            student.studentId === unenrolledId ? { ...student, whatsapp: serverPhone } : student,
          ),
          events: [...accepted.values(), ...notes.values()],
        },
      })
    if (path === '/api/student/sync') {
      const { operation } = offlineStudentSyncSchema.parse(route.request().postDataJSON())
      if (operation.kind === 'note') {
        const { id } = operation.event
        studentAttempts.push(id)
        notes.set(id, { ...operation.event, receivedAt: now })
        if (!lostNoteReply) {
          lostNoteReply = true
          return route.abort('connectionreset')
        }
        return route.fulfill({ json: { id, kind: 'saved', currentWhatsapp: serverPhone } })
      }
      const { id, expectedWhatsapp, whatsapp } = operation
      if (!contactReceipts.has(id)) {
        if (expectedWhatsapp !== serverPhone)
          return route.fulfill({ json: { id, kind: 'conflict', currentWhatsapp: serverPhone } })
        expect(whatsapp).toBe(updatedPhone)
        serverPhone = updatedPhone
        contactReceipts.add(id)
      }
      return route.fulfill({ json: { id, kind: 'saved', currentWhatsapp: serverPhone } })
    }
    expect(path).toBe('/api/attendance/sync')
    const body = offlineAttendanceSyncSchema.parse(route.request().postDataJSON())
    expect(body.accountId).toBe(accountId)
    let drop = false
    const outcomes = body.records.map(({ event, expectedEventId }) => {
      attempts.push(event.id)
      if (rejectFirstStudent && event.payload.studentId === studentIds[0])
        return { id: event.id, kind: 'forbidden', current: null }
      if (accepted.has(event.id))
        return {
          id: event.id,
          kind: 'saved',
          current: { eventId: event.id, status: event.payload.status },
        }
      if (event.payload.studentId === studentIds[1] && expectedEventId !== conflictingId)
        return {
          id: event.id,
          kind: 'conflict',
          current: { eventId: conflictingId, status: 'excused' },
        }
      accepted.set(event.id, { ...event, receivedAt: now })
      if (event.payload.studentId === studentIds[0] && !lostReply) {
        lostReply = true
        drop = true
      }
      return {
        id: event.id,
        kind: 'saved',
        current: { eventId: event.id, status: event.payload.status },
      }
    })
    if (drop) return route.abort('connectionreset')
    return route.fulfill({ json: { outcomes } })
  })

  await page.goto(courseUrl)
  await page.evaluate(() => navigator.serviceWorker.ready)
  await page.reload()
  await expect
    .poll(() => page.evaluate(() => Boolean(navigator.serviceWorker.controller)))
    .toBe(true)
  const onlineHeading = page.getByRole('heading', { name: 'Attendance & notes' })
  await expect(onlineHeading).toBeVisible()
  await expect
    .poll(() => page.evaluate(() => Boolean(localStorage.getItem('halaqa-offline-access'))))
    .toBe(true)

  await test.step('keeps the same attendance form and draft during a stale offline signal', async () => {
    await page.addInitScript(() => {
      if (sessionStorage.getItem('stale-offline-status'))
        Object.defineProperty(navigator, 'onLine', { configurable: true, value: false })
    })
    await page.getByRole('button', { name: `Add note for ${names[0]}`, exact: true }).click()
    const draft = page.getByRole('dialog').getByRole('textbox', { name: 'Note', exact: true })
    await draft.fill('Unsaved draft')
    networkAvailable = false
    await page.evaluate(() => {
      Object.defineProperty(navigator, 'onLine', { configurable: true, value: false })
      window.dispatchEvent(new Event('offline'))
    })
    await expect(page.getByText('Offline', { exact: true })).toBeVisible()
    await expect(onlineHeading).toBeVisible()
    await expect(draft).toHaveValue('Unsaved draft')
    networkAvailable = true
    await expect(page.getByText('Offline', { exact: true })).toHaveCount(0, { timeout: 15_000 })
    await expect(draft).toHaveValue('Unsaved draft')
    expect(await page.evaluate(() => navigator.onLine)).toBe(false)
    await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click()

    networkAvailable = false
    await page.evaluate(() => {
      localStorage.setItem('halaqa-language-preference', 'ar')
      const key = 'halaqa-offline-access'
      const access = JSON.parse(localStorage.getItem(key) ?? 'null')
      delete access.teacher
      localStorage.setItem(key, JSON.stringify(access))
      sessionStorage.setItem('stale-offline-status', '1')
    })
    await page.reload()
    await expect(page.getByRole('heading', { name: 'الحضور والملاحظات' })).toBeVisible()
    await expect(page.getByText('غير متصل', { exact: true })).toBeVisible()
    await expect(page.getByRole('button', { name: 'حاول مجدداً' })).toBeInViewport()
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    )
    networkAvailable = true
    await expect(page.getByText('غير متصل', { exact: true })).toHaveCount(0, { timeout: 15_000 })
    expect(await page.evaluate(() => navigator.onLine)).toBe(false)
    await page.evaluate(() => {
      sessionStorage.removeItem('stale-offline-status')
      Reflect.deleteProperty(navigator, 'onLine')
    })
    await page.getByRole('button', { name: 'الملف الشخصي', exact: true }).click()
    await page.getByRole('button', { name: 'English', exact: true }).click()
    await page.keyboard.press('Escape')
  })

  networkAvailable = false
  await context.setOffline(true)
  await expect(page.getByText('Offline', { exact: true })).toBeVisible()
  const offlinePage = await context.newPage()
  await offlinePage.addInitScript(() => {
    const actual = Date.now
    Date.now = () => actual() + 25 * 60 * 60 * 1000
  })
  await page.close()
  await offlinePage.goto(courseUrl)
  expect(await offlinePage.evaluate(() => navigator.onLine)).toBe(false)
  const navigation = offlinePage.getByRole('navigation', { name: 'Main navigation' })
  await expect(navigation).toBeVisible()
  await expect(offlinePage.getByRole('heading', { name: 'Attendance & notes' })).toBeVisible()
  await expect(offlinePage.getByText('Offline', { exact: true })).toBeVisible()
  const firstRow = offlinePage.getByRole('group', { name: `Attendance for ${names[0]}` })
  const secondRow = offlinePage.getByRole('group', { name: `Attendance for ${names[1]}` })
  await firstRow.getByRole('button', { name: /present/i }).click()
  await secondRow.getByRole('button', { name: /absent/i }).click()
  await offlinePage.getByRole('button', { name: 'Save attendance (2)' }).click()
  await expect(offlinePage.getByText(/2 pending changes/)).toBeVisible()

  const rosterUrl = '/?page=app&tab=roster&course=all'
  const openUnenrolledStudent = async () => {
    await offlinePage.goto(rosterUrl)
    const mobileOpen = offlinePage.getByRole('button', {
      name: `Open student: ${unenrolledName}`,
    })
    const desktopOpen = offlinePage
      .getByRole('row')
      .filter({ hasText: unenrolledName })
      .getByRole('button', { name: 'Open student', exact: true })
    await mobileOpen.or(desktopOpen).filter({ visible: true }).click()
  }
  await offlinePage.goto('/?page=app&view=profile')
  await expect(navigation).toBeVisible()
  await expect(offlinePage.getByText('Connect to Wartaq to open this page.')).toBeVisible()
  await expect(offlinePage).toHaveURL(/view=profile/)
  await openUnenrolledStudent()
  const profile = offlinePage.getByRole('button', { name: 'Profile', exact: true })
  await profile.click()
  await expect(offlinePage.getByRole('button', { name: 'Profile settings' })).toBeDisabled()
  await offlinePage.keyboard.press('Escape')
  const noteField = offlinePage.getByRole('textbox', { name: 'Note', exact: true })
  const phone = offlinePage.getByRole('textbox', { name: 'WhatsApp number' })
  for (const note of ['First offline note', 'Second offline note']) {
    await offlinePage.getByRole('button', { name: 'Add note', exact: true }).click()
    await noteField.fill(note)
    await offlinePage.getByRole('button', { name: 'Save note' }).click()
    await expect(offlinePage.getByText(note, { exact: true })).toBeVisible()
  }
  await offlinePage.getByRole('button', { name: /^Edit (student|phone)$/ }).click()
  await expect(phone).toHaveValue(oldPhone)
  await phone.fill(updatedPhone)
  await offlinePage.getByRole('button', { name: 'Save', exact: true }).click()
  await expect(offlinePage.getByText(/5 pending changes/)).toBeVisible()
  expect(
    await offlinePage.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
  ).toBe(true)
  await offlinePage.reload()
  await expect(offlinePage.getByText('First offline note', { exact: true })).toHaveCount(1)
  await expect(offlinePage.getByText('Second offline note', { exact: true })).toHaveCount(1)
  await offlinePage.getByRole('button', { name: /^Edit (student|phone)$/ }).click()
  await expect(phone).toHaveValue(updatedPhone)
  await offlinePage.goto(courseUrl)
  await expect(firstRow.getByRole('button', { name: /present/i })).toHaveAttribute(
    'aria-pressed',
    'true',
  )
  await expect(secondRow.getByRole('button', { name: /absent/i })).toHaveAttribute(
    'aria-pressed',
    'true',
  )
  expect(attempts).toEqual([])
  expect(studentAttempts).toEqual([])

  serverPhone = otherPhone
  networkAvailable = true
  await context.setOffline(false)
  const changesMenuItem = offlinePage.getByRole('button', { name: 'My changes', exact: true })
  const changesHeading = offlinePage.getByRole('heading', { name: 'My changes', exact: true })
  await expect(offlinePage.getByText(/Offline.*pending changes/)).toHaveCount(0)
  await profile.click()
  await expect(changesMenuItem).toBeVisible()
  await offlinePage.getByRole('button', { name: 'العربية', exact: true }).click()
  await offlinePage.getByRole('button', { name: 'تغييراتي', exact: true }).click()
  await expect(offlinePage.getByRole('heading', { name: 'تغييراتي', exact: true })).toBeVisible()
  await expect(offlinePage.locator('.workspace-rail nav')).toBeVisible()
  await expect(offlinePage.locator('.workspace-auth')).toHaveCount(0)
  await expect(offlinePage.getByRole('region', { name: 'التغييرات المنتظرة' })).toBeVisible()
  expect(
    await offlinePage.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
  ).toBe(true)
  await testInfo.attach('Arabic pending changes', {
    body: await offlinePage.screenshot({ animations: 'disabled' }),
    contentType: 'image/png',
  })
  await offlinePage.getByRole('button', { name: 'الملف الشخصي', exact: true }).click()
  await offlinePage.getByRole('button', { name: 'English', exact: true }).click()
  await offlinePage.keyboard.press('Escape')
  await expect(changesHeading).toBeVisible()
  await expect(offlinePage.getByRole('button', { name: 'Use my phone' })).toBeVisible()
  await expect(offlinePage.getByText('3 pending sync')).toBeVisible()
  expect(notes.size).toBe(2)
  expect(new Set(studentAttempts).size).toBe(2)
  expect(studentAttempts).toHaveLength(3)

  rejectFirstStudent = false
  await offlinePage.getByRole('button', { name: 'Sync now' }).click()
  await expect(offlinePage.getByText('2 pending sync')).toBeVisible()
  expect(accepted.size).toBe(1)
  expect(attempts.filter((id) => id === [...accepted.keys()][0]).length).toBeGreaterThanOrEqual(2)
  await offlinePage.getByRole('button', { name: 'Use my mark' }).click()
  await expect(offlinePage.getByText('1 pending sync')).toBeVisible()
  await offlinePage.getByRole('button', { name: 'Use my phone' }).click()
  await expect(offlinePage.getByText('No changes waiting to sync.')).toBeVisible()
  expect([...accepted.values()].map(({ payload }) => payload.status)).toEqual(['present', 'absent'])
  expect(serverPhone).toBe(updatedPhone)
  expect(contactReceipts.size).toBe(1)
  await expect(offlinePage.getByRole('button', { name: 'Sync now' })).toHaveCount(0)
  await expect(navigation).toBeVisible()
  await offlinePage.getByRole('button', { name: 'Back', exact: true }).click()
  await expect(navigation.getByRole('button', { name: 'Attendance', exact: true })).toHaveAttribute(
    'aria-current',
    'page',
  )

  snapshot.students.shift()
  snapshot.enrollments.shift()
  await offlinePage.evaluate(() => localStorage.removeItem('halaqa-offline-access'))
  await offlinePage.reload()
  await expect(offlinePage.getByRole('heading', { name: 'Attendance & notes' })).toBeVisible()
  await expect
    .poll(() => offlinePage.evaluate(() => Boolean(localStorage.getItem('halaqa-offline-access'))))
    .toBe(true)
  networkAvailable = false
  await context.setOffline(true)
  await expect(offlinePage.getByText('Offline', { exact: true })).toBeVisible()
  await expect(firstRow).toHaveCount(0)
  const clockStart = Date.now()
  await offlinePage.clock.install({ time: clockStart })
  await offlinePage.clock.pauseAt(clockStart + 1_000)
  await offlinePage.goto(`/?page=app&view=student&course=all&student=${unenrolledId}`)
  await offlinePage.clock.runFor(500)
  await expect(offlinePage.getByText('First offline note', { exact: true })).toHaveCount(1)
  await offlinePage.evaluate(() => localStorage.removeItem('halaqa-offline-access'))
  await offlinePage.clock.runFor(1_000)
  await expect(offlinePage.getByRole('alert')).toHaveText(
    'Offline access expired. Connect and sign in again.',
  )
  await expect(offlinePage.getByText('First offline note', { exact: true })).toHaveCount(0)
})
