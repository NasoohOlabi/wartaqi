import { readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { performance } from 'node:perf_hooks'
import { setTimeout as pause } from 'node:timers/promises'
import { chromium, devices, expect } from '@playwright/test'
import { startMeter } from './meter.mjs'
import { sampleResources } from './resources.mjs'

const [label, network, ...ports] = process.argv.slice(2)
if (
  !label ||
  !['hplus', 'hplus-warm', 'hplus-admin', 'hplus-dashboard', 'lan', 'lan-preparation'].includes(
    network,
  ) ||
  ports.length !== 2
)
  throw new Error('Usage: node scripts/performance/journey.mjs LABEL NETWORK PROXY_PORT APP_PORT')
if (!/^[a-z0-9-]+$/.test(label)) throw new Error('Use a lowercase report label')
const [proxyPort, appPort] = ports.map(Number)
const origin = 'http://127.0.0.1:' + proxyPort
const output = '.artifacts/performance/'
const password = (await readFile(output + 'fixture-password.local', 'utf8')).trim()
const courseHash = createHash('md5').update('course-1').digest('hex')
const course = [
  courseHash.slice(0, 8),
  courseHash.slice(8, 12),
  '4' + courseHash.slice(13, 16),
  '8' + courseHash.slice(17, 20),
  courseHash.slice(20),
].join('-')
const date = '2026-11-21'
const name = 'Fixture learner 0001'
const net = network.startsWith('hplus')
  ? { offline: false, latency: 600, downloadThroughput: 50_000, uploadThroughput: 12_500 }
  : null
const steps = []
const meter = await startMeter(proxyPort, appPort)
const browser = await chromium.launch()
const contextOptions = {
  ...devices['iPhone 13'],
  baseURL: origin,
  locale: 'en-US',
  timezoneId: 'Asia/Damascus',
}

async function device() {
  const context = await browser.newContext(contextOptions)
  context.setDefaultTimeout(75_000)
  await context.addInitScript(() => localStorage.setItem('halaqa-language-preference', 'en'))
  const page = await context.newPage()
  if (net) {
    const cdp = await context.newCDPSession(page)
    await cdp.send('Network.enable')
    await cdp.send('Network.emulateNetworkConditions', net)
    await cdp.send('Emulation.setCPUThrottlingRate', { rate: 2 })
  }
  return { context, page }
}

async function step(label, action) {
  console.log(label)
  meter.phase(label)
  const start = performance.now()
  try {
    await action()
    steps.push({ label, ms: Math.round(performance.now() - start), passed: true })
  } catch (error) {
    steps.push({
      label,
      ms: Math.round(performance.now() - start),
      passed: false,
      error: String(error).slice(0, 1200),
    })
    throw error
  }
}

async function signIn(page, destination, phone, ready) {
  await page.goto(destination, { waitUntil: 'domcontentloaded' })
  await page.getByLabel('Phone number', { exact: true }).fill(phone)
  await page.getByLabel('Password', { exact: true }).fill(password)
  await page.getByRole('button', { name: 'Sign in', exact: true }).click()
  await ready()
}

const checking = expect.configure({ timeout: 60_000 })
let stopMeasurement = await sampleResources()
let activePhase = null
const attendanceUrl = '/?page=app&tab=attendance&course=' + course + '&date=' + date
const classRow = (teacher) => teacher.getByLabel('Attendance for ' + name)
const learnerMark = (learner, status) =>
  learner
    .getByRole('row')
    .filter({ has: learner.getByText(date, { exact: true }) })
    .getByText(status, { exact: true })

try {
  if (network === 'lan-preparation') {
    const administrator = await device()
    const dashboard = administrator.page.getByRole('region', { name: 'Dashboard' })
    await step('administrator-prepares-full-offline-notebook', async () => {
      await signIn(
        administrator.page,
        '/?page=app&course=' + course + '&date=' + date,
        '+12025551000',
        () => checking(dashboard).toBeVisible(),
      )
      await checking
        .poll(() =>
          administrator.page.evaluate(() => Boolean(localStorage.getItem('halaqa-offline-access'))),
        )
        .toBe(true)
      await checking
        .poll(() =>
          administrator.page.evaluate(
            async () => (await navigator.serviceWorker.getRegistration())?.active?.state,
          ),
        )
        .toBe('activated')
    })
    await step('administrator-opens-cached-course-summary-offline', async () => {
      await administrator.context.setOffline(true)
      await administrator.page.reload({ waitUntil: 'domcontentloaded' })
      await checking(dashboard).toBeVisible()
      await administrator.page.getByRole('button', { name: 'Take attendance' }).click()
      await administrator.page.getByRole('button', { name: 'Summary', exact: true }).click()
      await checking(
        administrator.page.getByRole('button', { name: 'Export attendance chart' }),
      ).toBeEnabled()
    })
  } else if (network === 'hplus-dashboard') {
    const administrator = await device()
    await step('cold-administrator-dashboard', async () => {
      await signIn(
        administrator.page,
        '/?page=app&course=' + course + '&date=' + date,
        '+12025551000',
        () => checking(administrator.page.getByRole('region', { name: 'Dashboard' })).toBeVisible(),
      )
      await checking(administrator.page.getByText('Students to follow up')).toBeVisible()
    })
    await step('administrator-takes-attendance', async () => {
      await administrator.page.getByRole('button', { name: 'Take attendance' }).click()
      const present = classRow(administrator.page).getByRole('button', {
        name: 'present',
        exact: true,
      })
      await checking(present).toBeEnabled()
      const delivery = administrator.page.waitForResponse(
        (response) => new URL(response.url()).pathname === '/api/attendance/sync',
      )
      await present.click()
      const response = await delivery
      if (!response.ok() || (await response.json()).outcomes[0]?.kind !== 'saved')
        throw new Error('Administrator mark was not saved in Postgres')
      await checking(present).toHaveAttribute('aria-pressed', 'true')
      await administrator.page.reload({ waitUntil: 'domcontentloaded' })
      await checking(present).toHaveAttribute('aria-pressed', 'true')
    })
    await step('administrator-opens-course-summary', async () => {
      await administrator.page.getByRole('button', { name: 'Summary', exact: true }).click()
      const row = administrator.page.getByRole('row').filter({
        has: administrator.page.getByRole('rowheader', { name, exact: true }),
      })
      const totals = [1, 0, 0, 0]
      for (let day = 1; day <= 120; day++) {
        if (((120 - day) * 960 + 1) % 41 !== 0) {
          const category = (7 + day) % 10
          totals[category < 7 ? 0 : category < 9 ? 1 : 2]++
        }
        if (day % 7 === 0 && (1 + day) % 3 === 0) totals[3]++
      }
      await checking(row.getByRole('cell')).toHaveText(totals.map(String))
      await checking(
        administrator.page.getByRole('button', { name: 'Export attendance chart' }),
      ).toBeEnabled()
    })
    await step('administrator-opens-student-detail', async () => {
      await administrator.page.getByRole('button', { name: 'Session', exact: true }).click()
      await administrator.page.getByRole('button', { name, exact: true }).click()
      await checking(
        administrator.page.getByRole('heading', { name: 'Student notes' }),
      ).toBeVisible()
      await checking(
        administrator.page.getByText(/Synthetic follow-up for learner 1 on/).first(),
      ).toBeVisible()
    })
    await step('administrator-corrects-a-mark', async () => {
      await administrator.page.getByRole('button', { name: 'Back to attendance' }).click()
      await administrator.page.getByRole('button', { name: `Correct this mark: ${name}` }).click()
      await administrator.page
        .getByLabel('Reason', { exact: true })
        .fill('Synthetic administrator browser correction')
      const delivery = administrator.page.waitForResponse(
        (response) => new URL(response.url()).pathname === '/api/events/correct',
      )
      await administrator.page.getByRole('button', { name: 'Append correction' }).click()
      const response = await delivery
      if (!response.ok() || (await response.json()).saved !== 1)
        throw new Error('The correction was not saved in Postgres')
      await checking(
        classRow(administrator.page).getByRole('button', { name: 'present', exact: true }),
      ).toHaveAttribute('aria-pressed', 'false')
    })
    await step('administrator-pages-history', async () => {
      await administrator.page.goto('/?page=app&view=courses&courseView=history&course=' + course, {
        waitUntil: 'domcontentloaded',
      })
      await checking(
        administrator.page.getByRole('heading', { name: 'History & corrections' }),
      ).toBeVisible()
      await checking(
        administrator.page.getByText('Correction: Synthetic administrator browser correction', {
          exact: true,
        }),
      ).toBeVisible()
      await administrator.page.getByRole('button', { name: 'Older records', exact: true }).click()
      await checking(
        administrator.page.getByRole('button', { name: 'Newest records', exact: true }),
      ).toBeVisible()
      await checking(
        administrator.page.getByText('Correction: Synthetic administrator browser correction', {
          exact: true,
        }),
      ).toHaveCount(0)
    })
  } else {
    const staff = await device()
    const learner = await device()
    const administrator = await device()
    const present = classRow(staff.page).getByRole('button', { name: 'present', exact: true })
    const absent = classRow(staff.page).getByRole('button', { name: 'absent', exact: true })

    await step('cold-teacher', async () => {
      await signIn(staff.page, attendanceUrl, '+12025551001', () => checking(present).toBeEnabled())
    })
    await step('teacher-offline-preparation', async () => {
      await checking
        .poll(() =>
          staff.page.evaluate(
            async () => (await navigator.serviceWorker.getRegistration())?.active?.state,
          ),
        )
        .toBe('activated')
      await checking
        .poll(() =>
          staff.page.evaluate(() => Boolean(localStorage.getItem('halaqa-offline-access'))),
        )
        .toBe(true)
    })
    await step('cold-learner', async () => {
      await signIn(learner.page, '/?page=app', '+12025552001', () =>
        checking(learner.page.getByRole('heading', { name: 'My attendance' })).toBeVisible(),
      )
      await checking(
        learner.page.getByText('Fixture course 1', { exact: true }).first(),
      ).toBeVisible()
    })
    if (network === 'hplus-warm') {
      await step('idle-after-sign-in', () => pause(95_000))
      await stopMeasurement()
      stopMeasurement = await sampleResources()
      activePhase = 'online-mark-propagation'
    }
    await step('online-mark-propagation', async () => {
      await checking(present).toHaveAttribute('aria-pressed', 'false')
      await present.click()
      await checking(present).toHaveAttribute('aria-pressed', 'true')
      await checking(learnerMark(learner.page, 'present')).toBeVisible()
    })
    await step('teacher-note', async () => {
      await staff.page.getByRole('button', { name: 'Add note for ' + name, exact: true }).click()
      const dialog = staff.page.getByRole('dialog', { name: 'Student note for ' + name })
      await dialog
        .getByLabel('Note', { exact: true })
        .fill('Private synthetic follow-up for performance check.')
      const saved = staff.page.waitForResponse(
        (response) => new URL(response.url()).pathname === '/api/student/sync',
      )
      await dialog.getByRole('button', { name: 'Save note', exact: true }).click()
      if (!(await saved).ok()) throw new Error('Synthetic note was not saved')
    })
    await step('warm-reload', async () => {
      await staff.page.reload({ waitUntil: 'domcontentloaded' })
      await checking(present).toHaveAttribute('aria-pressed', 'true')
      await checking
        .poll(() => staff.page.evaluate(() => Boolean(navigator.serviceWorker.controller)))
        .toBe(true)
    })
    await step('offline-reload-and-queue', async () => {
      await staff.context.setOffline(true)
      await staff.page.reload({ waitUntil: 'domcontentloaded' })
      await checking(present).toHaveAttribute('aria-pressed', 'true')
      await absent.click()
      await checking(staff.page.getByText('Saved on this device. Waiting to sync.')).toBeVisible()
      await staff.page.reload({ waitUntil: 'domcontentloaded' })
      await checking(absent).toHaveAttribute('aria-pressed', 'true')
      await staff.context.setOffline(false)
    })
    await step('reconnect-and-delivery', async () => {
      await checking(learnerMark(learner.page, 'absent')).toBeVisible()
      await checking(
        learner.page.getByText('Private synthetic follow-up', { exact: false }),
      ).toHaveCount(0)
    })
    if (network === 'lan' || network === 'hplus-admin') {
      await step('cold-administrator', async () => {
        await signIn(administrator.page, '/?page=app&view=settings', '+12025551000', () =>
          checking(administrator.page.getByRole('heading', { name: 'Settings' })).toBeVisible(),
        )
        await checking(
          administrator.page.getByRole('link', { name: 'Students', exact: true }),
        ).toBeVisible()
      })
    }
  }
} finally {
  await browser.close()
  await meter.close()
  const resources = await stopMeasurement()
  await writeFile(
    output + label + '-' + network + '.json',
    JSON.stringify(
      { network, net, steps, activePhase, requests: meter.samples, resources },
      null,
      2,
    ) + '\n',
  )
}
