import { clickAccountAction } from './profileActions'
import { expect, test, type Page } from '@playwright/test'
import { compactHeader } from './headerLayout'
import { mkdir } from 'node:fs/promises'
import { makeEvent, type LedgerEvent } from '../shared/ledger'
import { offlineAttendanceSyncSchema } from '../shared/offlineAttendance'
import { acceptSyntheticSync } from './syntheticSync'

test.use({ trace: 'off' })

const courseId = '40000000-0000-4000-8000-000000000001'
const enrollmentFor = (studentId: string) => ({
  id: studentId,
  courseId,
  studentId,
  level: '',
  practical: null,
  theory: null,
  active: true,
})
const courses = [{ id: courseId, name: 'Synthetic course', open: true }]

let auth = ''
let api = ''
const users = [
  {
    id: '10000000-0000-4000-8000-000000000001',
    name: 'Teacher Alpha',
    email: 'alpha@example.test',
    phone: '+12025550121',
    student: 'Alpha Student',
    studentId: '20000000-0000-4000-8000-000000000001',
  },
  {
    id: '10000000-0000-4000-8000-000000000002',
    name: 'Teacher Beta',
    email: 'beta@example.test',
    phone: '+12025550122',
    student: 'Beta Student',
    studentId: '20000000-0000-4000-8000-000000000002',
  },
]
let origin = ''

test.beforeAll(({ browserName }, testInfo) => {
  if (browserName !== 'chromium') throw new Error('Synthetic test requires Chromium')
  const baseURL = testInfo.project.use.baseURL
  if (!baseURL) throw new Error('Synthetic test requires the suite preview baseURL')
  origin = baseURL
  auth = `${origin}/api/auth`
  api = `${origin}/api`
})

test('Arabic authentication keeps mobile input semantics and localized feedback', async ({
  page,
}) => {
  let submitted: unknown
  await page.setViewportSize({ width: 320, height: 568 })
  await page.route(`${auth}/**`, async (route) => {
    const headers = {
      'access-control-allow-origin': origin,
      'access-control-allow-credentials': 'true',
      'access-control-allow-methods': 'GET, POST, OPTIONS',
      'access-control-allow-headers':
        route.request().headers()['access-control-request-headers'] ?? 'content-type',
    }
    if (route.request().method() === 'OPTIONS') return route.fulfill({ status: 204, headers })
    if (route.request().url().includes('/sign-in/phone')) {
      submitted = route.request().postDataJSON()
      return route.fulfill({
        status: 401,
        headers,
        json: { code: 'INVALID_EMAIL_OR_PASSWORD', message: 'Invalid email or password' },
      })
    }
    return route.fulfill({ headers, json: null })
  })
  await page.goto(origin)
  await page.getByRole('button', { name: 'Sign in', exact: true }).click()
  await compactHeader(page, '.workspace-auth-header')
  await clickAccountAction(page, 'العربية')
  await compactHeader(page, '.workspace-auth-header')
  const phone = page.getByLabel('رقم الهاتف', { exact: true })
  const password = page.getByLabel('كلمة المرور', { exact: true })
  await expect(phone).toHaveAttribute('type', 'tel')
  await expect(phone).toHaveAttribute('autocomplete', 'username')
  await expect(password).toHaveAttribute('autocomplete', 'current-password')
  await expect(page.locator('form input')).toHaveCount(2)
  await page.locator('form input').evaluateAll((fields) => {
    // Some password managers fill the DOM without dispatching React change events.
    for (const field of fields) {
      if (field instanceof HTMLInputElement)
        field.value = field.type === 'tel' ? '0912345678' : 'synthetic-password'
    }
  })
  await password.press('Enter')
  await expect(page.getByRole('alert')).toBeVisible()
  expect(submitted).toEqual({ phone: '0912345678', password: 'synthetic-password' })
  const form = page.getByRole('form', { name: 'تسجيل الدخول' })
  await expect(form).toHaveAttribute('method', 'post')
  await expect(form.getByLabel('رقم الهاتف', { exact: true })).toHaveAttribute('name', 'username')
  await expect(phone).toHaveAccessibleDescription('يمكن أن يبدأ الرقم السوري بـ 09 أو +9639.')
  await expect(password).toHaveAttribute('id', 'current-password')
  await expect(password).toHaveAttribute('name', 'password')
  await clickAccountAction(page, 'English')
  await expect(page.getByLabel('Phone number', { exact: true })).toHaveValue('0912345678')
  await expect(page.getByRole('alert')).toContainText('Invalid phone number or password')
  await expect(page.getByText('Syrian numbers can start with 09 or +9639.')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Create account', exact: true })).toHaveCount(0)
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
    true,
  )
})

test('sign in, reload and switch teacher without mixing notebook data', async ({
  context,
  page,
}) => {
  let current: (typeof users)[number] | undefined
  const events: Record<string, LedgerEvent[]> = {}
  const attempts = new Map<string, number>()
  let releaseApproval = () => {}
  const approvalHeld = new Promise<void>((resolve) => {
    releaseApproval = resolve
  })
  let firstApproval = true
  await context.route(`${auth}/**`, async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname
    const headers = {
      'access-control-allow-origin': origin,
      'access-control-allow-credentials': 'true',
    }
    if (request.method() === 'OPTIONS')
      return route.fulfill({
        status: 204,
        headers: {
          ...headers,
          'access-control-allow-methods': 'GET, POST, OPTIONS',
          'access-control-allow-headers':
            request.headers()['access-control-request-headers'] ?? 'content-type',
        },
      })
    if (path.endsWith('/sign-in/phone')) {
      const body = request.postDataJSON()
      current = users.find((user) => user.phone === body.phone)
    }
    if (path.endsWith('/sign-out')) {
      current = undefined
      return route.fulfill({ json: { success: true }, headers })
    }
    const session = current
      ? {
          user: {
            id: current.id,
            name: current.name,
            email: current.email,
            emailVerified: true,
            createdAt: new Date().toISOString(),
            updatedAt: new Date().toISOString(),
          },
          session: {
            id: 'synthetic-session',
            userId: current.id,
            token: 'opaque-session',
            expiresAt: new Date(Date.now() + 3600000).toISOString(),
            createdAt: new Date().toISOString(),
            updatedAt: new Date().toISOString(),
          },
        }
      : null
    return route.fulfill({ json: session, headers })
  })
  await context.route(`${api}/**`, async (route) => {
    if (new URL(route.request().url()).pathname.startsWith('/api/auth/')) return route.fallback()
    const request = route.request()
    const headers = {
      'access-control-allow-origin': origin,
      'access-control-allow-credentials': 'true',
    }
    if (request.method() === 'OPTIONS')
      return route.fulfill({
        status: 204,
        headers: {
          ...headers,
          'access-control-allow-methods': 'GET, POST, OPTIONS',
          'access-control-allow-headers':
            request.headers()['access-control-request-headers'] ?? 'content-type',
        },
      })
    const selected = users.find(
      (user) => user.id === request.headers()['x-user-id'] && user.id === current?.id,
    )
    if (!selected) return route.fulfill({ status: 401, json: { error: 'Unauthorized' }, headers })
    const teacher = {
      role: 'teacher',
      id: selected.id,
      name: selected.name,
      canRead: true,
      canWrite: selected === users[0],
      expiresAt: null,
    }
    if (request.url().endsWith('/api/me')) {
      if (firstApproval) {
        firstApproval = false
        await approvalHeld
      }
      return route.fulfill({ json: { account: teacher, teacher }, headers })
    }
    if (request.method() === 'POST') {
      const ledger = (events[selected.id] ??= [])
      const lengthBefore = ledger.length
      const receipt = acceptSyntheticSync(
        new URL(request.url()).pathname,
        request.postDataJSON(),
        ledger,
      )
      const id = receipt?.outcomes?.[0]?.id ?? receipt?.id
      if (!id) throw new Error('Expected a sync receipt')
      attempts.set(id, (attempts.get(id) ?? 0) + 1)
      if (ledger.length > lengthBefore)
        return route.fulfill({ status: 503, json: { error: 'Synthetic response loss' }, headers })
      return route.fulfill({ json: receipt, headers })
    }
    return route.fulfill({
      json: {
        teacher,
        students: [
          {
            kind: 'student',
            studentId: selected.studentId,
            name: selected.student,
            group: '',
            active: true,
          },
        ],
        events: events[selected.id] ?? [],
        courses,
        enrollments: [enrollmentFor(selected.studentId)],
      },
      headers,
    })
  })
  await page.goto(`${origin}/?page=app`)
  await page.getByLabel('Phone number').fill(users[0].phone)
  await page.getByLabel('Password').fill('synthetic-password')
  await page.getByRole('button', { name: 'Sign in', exact: true }).click()
  await expect(page.getByText('Checking your access…')).toBeVisible()
  await expect(page.getByText('Alpha Student')).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Explore with sample students' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Sign out' })).toHaveCount(0)
  releaseApproval()
  await expect(page.getByRole('heading', { name: 'Attendance & notes' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Add student', exact: true })).toHaveCount(0)
  await expect(page.getByRole('combobox', { name: 'Default course', exact: true })).toHaveCount(0)
  await page.getByRole('button', { name: 'Take attendance' }).click()
  await page.getByLabel('Change course').click()
  await page.getByRole('button', { name: 'Synthetic course', exact: true }).click()
  await expect(page.getByText('Alpha Student')).toBeVisible()
  await expect(page.getByText('Beta Student')).toHaveCount(0)
  await page
    .getByLabel('Attendance for Alpha Student')
    .getByRole('button', { name: 'present', exact: true })
    .click()
  await expect(page.getByText('Saved on this device. Waiting to sync.')).toBeVisible()
  await expect(page.getByText('Changes accepted by Wartaq.')).toBeVisible()
  expect(events[users[0].id]).toHaveLength(1)
  expect(attempts.get(events[users[0].id][0].id)).toBe(2)
  const sessionDate = '2026-11-18'
  await page.goto(`${origin}/?page=app&tab=notes&course=${courseId}&date=${sessionDate}&tag=legacy`)
  await expect(page.getByLabel('Session date')).toHaveValue(sessionDate)
  const addNote = page.getByRole('button', { name: 'Add note for Alpha Student' })
  await addNote.click()
  const dialog = page.getByRole('dialog', { name: 'Student note for Alpha Student' })
  await dialog.getByLabel('Note', { exact: true }).fill('Synthetic attendance note')
  await dialog.getByRole('button', { name: 'Save note' }).click()
  await expect(dialog).toBeHidden()
  await expect(page.getByText('Changes accepted by Wartaq.')).toBeVisible()
  expect(events[users[0].id]).toHaveLength(2)
  expect(attempts.get(events[users[0].id][1].id)).toBe(2)
  expect(events[users[0].id].map(({ payload }) => payload).slice(1)).toEqual([
    {
      kind: 'note',
      studentId: users[0].studentId,
      date: sessionDate,
      tags: ['attendance'],
      text: 'Synthetic attendance note',
    },
  ])
  await page.reload()
  await page.getByRole('button', { name: 'Alpha Student', exact: true }).click()
  await expect(
    page.getByRole('list', { name: 'Student notes' }).getByText('Synthetic attendance note'),
  ).toBeVisible()
  let releasePreviousRequest = () => {}
  const previousRequest = new Promise<void>((resolve) => {
    releasePreviousRequest = resolve
  })
  await page.route(
    `${api}/notebook`,
    async (route) => {
      await previousRequest
      await route.fulfill({ status: 403, json: { error: 'Previous account is no longer active' } })
    },
    { times: 1 },
  )
  await page.getByRole('navigation').getByRole('button', { name: 'Students' }).click()
  const refreshing = page.waitForRequest(`${api}/notebook`)
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await refreshing
  await clickAccountAction(page, 'Profile settings')
  await clickAccountAction(page, 'Sign out')
  await expect(page.locator('.wartaq-home h1')).toBeVisible()
  await page.getByRole('button', { name: 'Sign in', exact: true }).click()
  await page.getByLabel('Phone number').fill(users[1].phone)
  await page.getByLabel('Password').fill('synthetic-password')
  await page.getByRole('button', { name: 'Sign in', exact: true }).click()
  await page.getByRole('button', { name: 'Take attendance' }).click()
  await page.getByLabel('Change course').click()
  await page.getByRole('button', { name: 'Synthetic course', exact: true }).click()
  const denied = page.waitForResponse(
    (response) => response.url() === `${api}/notebook` && response.status() === 403,
  )
  releasePreviousRequest()
  await denied
  await expect(page.getByText('Beta Student')).toBeVisible()
  await expect(page.getByText('Alpha Student')).toHaveCount(0)
  await expect(
    page
      .getByLabel('Attendance for Beta Student')
      .getByRole('button', { name: 'present', exact: true }),
  ).toBeDisabled()
  await page.getByRole('button', { name: 'Add note for Beta Student' }).click()
  const betaNote = page.getByRole('dialog', { name: 'Student note for Beta Student' })
  await expect(betaNote.getByLabel('Note', { exact: true })).toBeDisabled()
  await expect(betaNote.getByRole('button', { name: 'Save note' })).toBeDisabled()
  await expect(betaNote.getByText('Synthetic attendance note')).toHaveCount(0)
  expect(events[users[0].id]).toHaveLength(2)
  for (const route of ['view=history', 'tab=history']) {
    await page.goto(`${origin}/?page=app&${route}`)
    await expect(page.getByRole('button', { name: 'Take attendance' })).toBeVisible()
    await expect(page.getByRole('heading', { name: 'History & corrections' })).toHaveCount(0)
    await expect(page.locator('.workspace-rail nav').getByText(/History/)).toHaveCount(0)
    await expect(page).toHaveURL(/view=notebook/)
  }
})

test('session restoration shows a neutral placeholder until signed out', async ({ page }) => {
  let release = () => {}
  const held = new Promise<void>((resolve) => {
    release = resolve
  })
  await page.route(`${auth}/**`, async (route) => {
    await held
    await route.fulfill({ json: null }).catch(() => {})
  })
  await page.setViewportSize(
    test.info().project.name === 'mobile'
      ? { width: 320, height: 568 }
      : { width: 1280, height: 800 },
  )
  await page.goto(`${origin}/?page=app`)
  await expect(page.getByText('Loading your session…')).toBeVisible()
  await expect(page.getByLabel('Phone number')).toHaveCount(0)
  await expect(page.getByRole('button', { name: /Explore|Grow|Sign out/ })).toHaveCount(0)
  await mkdir('.artifacts/verification', { recursive: true })
  await page.screenshot({ path: `.artifacts/verification/loading-${test.info().project.name}.png` })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
    true,
  )
  release()
  await expect(page.getByLabel('Phone number')).toBeVisible()
})

test('Arabic session placeholder fits a narrow screen', async ({ page }) => {
  let release = () => {}
  const held = new Promise<void>((resolve) => {
    release = resolve
  })
  await page.addInitScript(() => localStorage.setItem('halaqa-language-preference', 'ar'))
  await page.route(`${auth}/**`, async (route) => {
    await held
    await route.fulfill({ json: null }).catch(() => {})
  })
  await page.setViewportSize({ width: 320, height: 568 })
  await page.goto(`${origin}/?page=app`)
  await expect(page.locator('html')).toHaveAttribute('dir', 'rtl')
  await mkdir('.artifacts/verification', { recursive: true })
  await page.screenshot({
    path: `.artifacts/verification/loading-ar-${test.info().project.name}.png`,
  })
  await expect(page.getByRole('status')).toBeVisible()
  await expect(page.getByLabel('Phone number')).toHaveCount(0)
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
    true,
  )
  release()
})

test('failed session check offers retry', async ({ page }) => {
  let fail = true
  await page.route(`${auth}/**`, async (route) => {
    if (fail) return route.fulfill({ status: 503, json: { message: 'Unavailable' } })
    return route.fulfill({ json: null })
  })
  await page.goto(`${origin}/?page=app`)
  await expect(page.getByRole('alert')).toContainText('Could not check your session')
  await expect(page.getByLabel('Phone number')).toHaveCount(0)
  fail = false
  await page.getByRole('button', { name: 'Try again' }).click()
  await expect(page.getByLabel('Phone number')).toBeVisible()
})

test('stalled session check times out and can recover', async ({ page }) => {
  let stall = true
  let release = () => {}
  const held = new Promise<void>((resolve) => {
    release = resolve
  })
  await page.route(`${auth}/**`, async (route) => {
    if (stall) await held
    await route
      .fulfill({
        headers: {
          'access-control-allow-origin': origin,
          'access-control-allow-credentials': 'true',
        },
        json: null,
      })
      .catch(() => {})
  })
  await page.goto(`${origin}/?page=app`)
  await expect(page.getByText('Could not check your session. Please try again.')).toBeVisible({
    timeout: 15000,
  })
  stall = false
  await page.getByRole('button', { name: 'Try again' }).click()
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeEnabled()
  release()
})

test('account retry ignores stale denial and notebook waits for its snapshot', async ({ page }) => {
  test.setTimeout(45000)
  const user = users[0]
  const now = new Date().toISOString()
  const teacher = {
    role: 'teacher',
    id: user.id,
    name: user.name,
    canRead: true,
    canWrite: true,
    expiresAt: null,
  }
  const session = {
    user: {
      id: user.id,
      name: user.name,
      email: user.email,
      emailVerified: true,
      createdAt: now,
      updatedAt: now,
    },
    session: {
      id: 'synthetic-session',
      userId: user.id,
      token: 'synthetic-token',
      expiresAt: new Date(Date.now() + 3600000).toISOString(),
      createdAt: now,
      updatedAt: now,
    },
  }
  let releaseAccount = () => {}
  let releaseNotebook = () => {}
  const accountHeld = new Promise<void>((resolve) => {
    releaseAccount = resolve
  })
  const notebookHeld = new Promise<void>((resolve) => {
    releaseNotebook = resolve
  })
  let accounts = 0
  await page.route(`${auth}/**`, (route) => route.fulfill({ json: session }))
  await page.route(`${api}/**`, async (route) => {
    if (new URL(route.request().url()).pathname.startsWith('/api/auth/')) return route.fallback()
    if (route.request().url().endsWith('/api/me')) {
      accounts++
      if (accounts === 1) {
        await accountHeld
        return route.fulfill({ status: 403 }).catch(() => {})
      }
      return route.fulfill({ json: { account: teacher } })
    }
    await notebookHeld
    return route
      .fulfill({
        json: {
          teacher,
          students: [
            {
              kind: 'student',
              studentId: user.studentId,
              name: user.student,
              group: '',
              active: true,
            },
          ],
          events: [],
          courses,
          enrollments: [enrollmentFor(user.studentId)],
        },
      })
      .catch(() => {})
  })
  await page.setViewportSize({ width: 320, height: 568 })
  await page.goto(`${origin}/?page=app&tab=attendance&course=${courseId}`)
  await expect(page.getByText('Checking your access\u2026')).toBeVisible()
  await expect(page.getByRole('button', { name: /Sign out|Explore|Grow/ })).toHaveCount(0)
  await expect(page.getByRole('alert')).toContainText('request failed', { timeout: 20000 })
  await page.getByRole('button', { name: 'Try again' }).click()
  await expect(page.getByText('Loading records\u2026')).toBeVisible()
  releaseAccount()
  await expect(page.getByRole('alert')).toHaveCount(0)
  await expect(page.getByText('Alpha Student')).toHaveCount(0)
  await expect(page.getByText('No students')).toHaveCount(0)
  releaseNotebook()
  await expect(page.getByLabel('Attendance for Alpha Student')).toBeVisible()
  expect(new URL(page.url()).searchParams.get('tab')).toBe('attendance')
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
    true,
  )
})

async function mockTeacher(page: Page, post: () => Promise<void>) {
  const user = users[0]
  const now = new Date().toISOString()
  const session = {
    user: {
      id: user.id,
      name: user.name,
      email: user.email,
      emailVerified: true,
      createdAt: now,
      updatedAt: now,
    },
    session: {
      id: 'synthetic-session',
      userId: user.id,
      token: 'synthetic-token',
      expiresAt: new Date(Date.now() + 3600000).toISOString(),
      createdAt: now,
      updatedAt: now,
    },
  }
  const teacher = {
    role: 'teacher',
    id: user.id,
    name: user.name,
    canRead: true,
    canWrite: true,
    expiresAt: null,
  }
  const events: LedgerEvent[] = []
  await page.route(`${auth}/**`, (route) => route.fulfill({ json: session }))
  await page.route(`${api}/**`, async (route) => {
    if (new URL(route.request().url()).pathname.startsWith('/api/auth/')) return route.fallback()
    if (route.request().url().endsWith('/api/me'))
      return route.fulfill({ json: { account: teacher } })
    if (route.request().method() === 'POST') {
      await post()
      const receipt = acceptSyntheticSync(
        new URL(route.request().url()).pathname,
        route.request().postDataJSON(),
        events,
      )
      if (receipt) return route.fulfill({ json: receipt })
    }
    return route.fulfill({
      json: {
        teacher,
        students: [
          {
            kind: 'student',
            studentId: user.studentId,
            name: user.student,
            group: '',
            active: true,
          },
        ],
        events,
        courses,
        enrollments: [enrollmentFor(user.studentId)],
      },
    })
  })
  await page.goto(`${origin}/?page=app`)
  await page.getByRole('button', { name: 'Take attendance' }).click()
  await page.getByLabel('Change course').click()
  await page.getByRole('button', { name: 'Synthetic course', exact: true }).click()
  await expect(page.getByLabel('Attendance for Alpha Student')).toBeVisible()
}

test('a warm notebook opens during a held read and coalesces a foreground burst', async ({
  page,
}) => {
  await mockTeacher(page, async () => {})
  await expect
    .poll(() => page.evaluate(() => !!localStorage.getItem('halaqa-offline-access')))
    .toBe(true)
  let release = () => {}
  const held = new Promise<void>((resolve) => {
    release = resolve
  })
  let reads = 0
  await page.route(`${api}/notebook`, async (route) => {
    reads += 1
    if (reads === 1) await held
    return route.fallback()
  })
  try {
    await page.reload()
    await expect.poll(() => reads).toBe(1)
    await expect(page.getByLabel('Attendance for Alpha Student')).toBeVisible()
    await page.evaluate(() => {
      for (let i = 0; i < 5; i += 1) window.dispatchEvent(new Event('focus'))
    })
    expect(reads).toBe(1)
    release()
    await expect.poll(() => reads).toBe(2)
  } finally {
    release()
  }
})

test('unconfirmed sign-out hides private records and offers a retry', async ({ page }) => {
  await mockTeacher(page, async () => {})
  let attempts = 0
  await page.route(`${auth}/sign-out`, (route) => {
    attempts++
    return attempts === 1
      ? route.fulfill({ status: 503, json: { message: 'Service unavailable' } })
      : route.fulfill({ json: { success: true } })
  })
  await clickAccountAction(page, 'Sign out')
  await expect(page.getByRole('alert')).toContainText('Could not confirm sign-out')
  await expect(page.getByText('Alpha Student')).toHaveCount(0)
  await expect(page.locator('.wartaq-home')).toHaveCount(0)
  await page.getByRole('button', { name: 'Sign out' }).click()
  await expect(page.locator('.wartaq-home h1')).toBeVisible()
  expect(attempts).toBe(2)
})

async function saveAttendance(page: Page, mark = 'present') {
  await page
    .getByLabel('Attendance for Alpha Student')
    .getByRole('button', { name: mark, exact: true })
    .click()
}

async function installLegacyBatch(page: Page) {
  const { id, name, studentId } = users[0]
  const event = makeEvent(name, {
    kind: 'attendance',
    studentId,
    enrollmentId: studentId,
    date: '2026-11-27',
    status: 'present',
  })
  await page.addInitScript(
    ({ id, event }) => {
      if (sessionStorage.getItem('synthetic-legacy-batch')) return
      sessionStorage.setItem('synthetic-legacy-batch', 'installed')
      localStorage.setItem(`halaqa-pending-neon:${location.origin}:${id}`, JSON.stringify([event]))
    },
    { id, event },
  )
}

test('storage write failure prevents authenticated writes', async ({ page }) => {
  let posts = 0
  await page.addInitScript(() => {
    const original = IDBDatabase.prototype.transaction
    IDBDatabase.prototype.transaction = function (stores, mode, options) {
      const transaction = original.call(this, stores, mode, options)
      if (
        mode === 'readwrite' &&
        typeof stores !== 'string' &&
        Array.from(stores).includes('pending')
      )
        queueMicrotask(() => transaction.abort())
      return transaction
    }
  })
  await mockTeacher(page, async () => {
    posts++
  })
  await saveAttendance(page)
  await expect(page.getByRole('alert').first()).toContainText(
    'Could not save these changes on this device',
  )
  const present = page
    .getByLabel('Attendance for Alpha Student')
    .getByRole('button', { name: 'present', exact: true })
  await expect(present).toBeEnabled()
  await expect(present).toHaveAttribute('aria-pressed', 'false')
  expect(posts).toBe(0)
})

test('old pending batches block authenticated writes', async ({ page }) => {
  let posts = 0
  await page.addInitScript(() => localStorage.setItem('halaqa-pending-v1:old', 'pending'))
  await mockTeacher(page, async () => {
    posts++
  })
  await expect(page.getByRole('alert').first()).toContainText(
    'Copy the pending batch from browser storage',
  )
  await expect(
    page
      .getByLabel('Attendance for Alpha Student')
      .getByRole('button', { name: 'present', exact: true }),
  ).toBeDisabled()
  expect(posts).toBe(0)
})

test('foreign pending storage also blocks retry of a restored batch', async ({ page }) => {
  let posts = 0
  await installLegacyBatch(page)
  await mockTeacher(page, async () => {})
  await page.route(`${api}/events`, (route) => {
    posts++
    return route.fulfill({ status: 503, json: { error: 'Synthetic response loss' } })
  })
  await expect(page.getByText('1 records awaiting confirmation')).toBeVisible()
  await page.evaluate(() => localStorage.setItem('halaqa-pending-v1:old', 'pending'))
  await page.reload()
  await expect(page.getByRole('alert').first()).toContainText('Copy the pending batch')
  await expect(page.getByRole('button', { name: 'Retry pending batch' })).toBeDisabled()
  expect(posts).toBe(0)
})

test('a foreign batch appearing after load blocks a new save', async ({ page }) => {
  let posts = 0
  await mockTeacher(page, async () => {
    posts++
  })
  await page.evaluate(() => localStorage.setItem('halaqa-pending-v1:old', 'pending'))
  await saveAttendance(page)
  await expect(page.getByRole('alert').first()).toContainText('Copy the pending batch')
  expect(posts).toBe(0)
})

test('corrupted pending storage blocks authenticated writes', async ({ page }) => {
  let posts = 0
  await page.addInitScript(
    (id) => localStorage.setItem(`halaqa-pending-neon:${location.origin}:${id}`, ''),
    users[0].id,
  )
  await mockTeacher(page, async () => {
    posts++
  })
  await expect(page.getByRole('alert').first()).toContainText('saved batch is corrupted')
  expect(posts).toBe(0)
})

test('successive marks queue individually while the first delivery is held', async ({ page }) => {
  let release = () => {}
  const gate = new Promise<void>((resolve) => {
    release = resolve
  })
  const requests: Array<ReturnType<typeof offlineAttendanceSyncSchema.parse>> = []
  page.on('request', (request) => {
    if (request.method() === 'POST' && request.url() === `${api}/attendance/sync`)
      requests.push(offlineAttendanceSyncSchema.parse(request.postDataJSON()))
  })
  try {
    await mockTeacher(page, async () => await gate)
    await saveAttendance(page)
    await expect.poll(() => requests.length).toBe(1)
    await saveAttendance(page, 'absent')
    await saveAttendance(page, 'absent')
    await saveAttendance(page, 'excused')
    await expect(
      page.getByLabel('Attendance for Alpha Student').getByRole('button', { pressed: true }),
    ).toHaveAccessibleName('excused')
    await expect(page.getByRole('button', { name: 'My changes', exact: true })).toBeVisible()
    expect(requests).toHaveLength(1)
    release()
    await expect.poll(() => requests.length).toBe(4)
    const records = requests.flatMap(({ records }) => records)
    expect(requests.every(({ records }) => records.length === 1)).toBe(true)
    expect(records.map(({ event }) => event.payload.status)).toEqual([
      'present',
      'absent',
      null,
      'excused',
    ])
    expect(records.map(({ expectedEventId }) => expectedEventId)).toEqual([
      null,
      records[0].event.id,
      records[1].event.id,
      records[2].event.id,
    ])
    await expect(page.getByText('Changes accepted by Wartaq.')).toBeVisible()
    await page.reload()
    await expect(
      page.getByLabel('Attendance for Alpha Student').getByRole('button', { pressed: true }),
    ).toHaveAccessibleName('excused')
  } finally {
    release()
  }
})

test('a second tab can queue locally while the first tab holds network delivery', async ({
  context,
  page,
}) => {
  let release = () => {}
  const gate = new Promise<void>((resolve) => {
    release = resolve
  })
  let posts = 0
  const write = async () => {
    posts++
    await gate
  }
  const second = await context.newPage()
  try {
    await mockTeacher(page, write)
    await mockTeacher(second, write)
    await saveAttendance(page)
    await expect.poll(() => posts).toBe(1)
    await saveAttendance(second, 'absent')
    await expect(second.getByText('Saved on this device. Waiting to sync.')).toBeVisible()
    expect(posts).toBe(1)
    release()
    await expect.poll(() => posts).toBe(2)
  } finally {
    release()
  }
})

test('a legacy account lock prevents a new client from sending', async ({ context, page }) => {
  let posts = 0
  const oldTab = await context.newPage()
  try {
    await mockTeacher(page, async () => {
      posts++
    })
    await oldTab.goto(origin)
    await oldTab.evaluate(async (id) => {
      await new Promise<void>((acquired) => {
        void navigator.locks.request(`halaqa-pending-neon:${location.origin}:${id}`, async () => {
          acquired()
          await new Promise<void>((release) =>
            window.addEventListener('release-test-lock', () => release(), { once: true }),
          )
        })
      })
    }, users[0].id)
    await saveAttendance(page)
    await expect(page.getByRole('alert')).toContainText('Another tab is saving')
    expect(posts).toBe(0)
  } finally {
    await oldTab.evaluate(() => window.dispatchEvent(new Event('release-test-lock')))
    await oldTab.close()
  }
})

test('a stale retry cannot recreate a batch cleared in another tab', async ({ page }) => {
  let posts = 0
  await installLegacyBatch(page)
  await mockTeacher(page, async () => {})
  await page.route(`${api}/events`, (route) => {
    posts++
    return route.fulfill({ status: 503, json: { error: 'Synthetic response loss' } })
  })
  await expect(page.getByText('1 records awaiting confirmation')).toBeVisible()
  await page.evaluate(
    (id) => localStorage.removeItem(`halaqa-pending-neon:${location.origin}:${id}`),
    users[0].id,
  )
  await page.getByRole('button', { name: 'Retry pending batch' }).click()
  await expect(page.getByRole('alert').first()).toContainText('Reload before retrying')
  expect(posts).toBe(0)
  expect(
    await page.evaluate(
      (id) => localStorage.getItem(`halaqa-pending-neon:${location.origin}:${id}`),
      users[0].id,
    ),
  ).toBeNull()
})

test('malformed notebook snapshots block saving', async ({ page }) => {
  let posts = 0
  await mockTeacher(page, async () => {
    posts++
  })
  await page.route(`${api}/notebook`, (route) =>
    route.fulfill({ json: { students: 'invalid', events: [] } }),
  )
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Students' })
    .click()
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('The request failed')
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Attendance' })
    .click()
  await expect(
    page
      .getByLabel('Attendance for Alpha Student')
      .getByRole('button', { name: 'present', exact: true }),
  ).toBeDisabled()
  await expect(page.getByRole('alert')).toContainText('The request failed')
  expect(posts).toBe(0)
})

test('pending retry rejects a student removed from the current assignment', async ({ page }) => {
  let posts = 0
  await installLegacyBatch(page)
  await mockTeacher(page, async () => {})
  await page.route(`${api}/events`, (route) => {
    posts++
    return route.fulfill({ status: 503, json: { error: 'Synthetic response loss' } })
  })
  await expect(page.getByText('1 records awaiting confirmation')).toBeVisible()
  const teacher = {
    role: 'teacher',
    id: users[0].id,
    name: users[0].name,
    canRead: true,
    canWrite: true,
    expiresAt: null,
  }
  await page.route(`${api}/notebook`, (route) =>
    route.fulfill({ json: { teacher, students: [], events: [] } }),
  )
  await page.getByRole('button', { name: 'Retry pending batch' }).click()
  await expect(page.getByRole('alert')).toContainText('outside your assigned active students')
  expect(posts).toBe(0)
})
