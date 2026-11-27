import { courseId, mockSyntheticNotebook, openNotebookHistory } from './syntheticNotebook'
import { clickRecordAction } from './recordActions'
import { clickAccountAction } from './profileActions'
import { expect, test } from '@playwright/test'
import { mkdir } from 'node:fs/promises'
import { offlineAttendanceSyncSchema } from '../shared/offlineAttendance'

const browserErrors = new WeakMap<object, string[]>()
test.beforeEach(async ({ context, page }) => {
  const errors: string[] = []
  browserErrors.set(context, errors)
  const watch = (tab: typeof page) => {
    tab.on('pageerror', (error) => errors.push(error.message))
    tab.on('console', (message) => {
      if (message.type() === 'error') errors.push(message.text())
    })
  }
  watch(page)
  context.on('page', watch)
})
test.afterEach(async ({ context, hasTouch }, testInfo) => {
  const errors = browserErrors.get(context) ?? []
  // The synthetic lost-response test deliberately aborts one network request.
  const unexpected = errors.filter((error) => {
    if (testInfo.title.includes('lost response') && error.includes('net::ERR_FAILED')) return false
    return !(
      hasTouch &&
      testInfo.title.includes('pressing saved attendance') &&
      error === 'Failed to load resource: net::ERR_INTERNET_DISCONNECTED'
    )
  })
  expect(unexpected).toEqual([])
})

test('authenticated notebook supports attendance, notes and corrections', async ({
  page,
}, testInfo) => {
  const external: string[] = []
  page.on('request', (request) => {
    if (new URL(request.url()).hostname !== '127.0.0.1') external.push(request.url())
  })
  await mockSyntheticNotebook(page, { names: ['أحمد', 'Omar', 'ليلى'] })
  await page.goto('/?page=app')
  await page.getByRole('button', { name: 'Attendance', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Notes', exact: true })).toHaveCount(0)
  const sessionDate = await page.getByLabel('Session date').inputValue()
  const marks = page.getByLabel('Attendance for ليلى')
  const present = marks.getByRole('button', { name: 'present', exact: true })
  const absent = marks.getByRole('button', { name: 'absent', exact: true })
  const excused = marks.getByRole('button', { name: 'excused', exact: true })
  const selected = marks.getByRole('button', { pressed: true })
  let attendancePosts = 0
  page.on('request', (request) => {
    if (request.method() === 'POST' && new URL(request.url()).pathname === '/api/attendance/sync')
      attendancePosts++
  })
  const tileBackgrounds = new Set<string>()
  const iconColors = new Set<string>()
  await expect(selected).toHaveCount(0)
  await present.focus()
  await page.keyboard.press('ArrowRight')
  await expect(absent).toBeFocused()
  await expect(selected).toHaveCount(0)
  for (const mark of [present, excused, absent]) {
    await mark.press('Space')
    await expect(mark).toHaveAttribute('aria-pressed', 'true')
    tileBackgrounds.add(await mark.evaluate((button) => getComputedStyle(button).backgroundColor))
    iconColors.add(await mark.locator('svg').evaluate((icon) => getComputedStyle(icon).color))
  }
  expect(tileBackgrounds.size).toBe(3)
  expect([...tileBackgrounds]).not.toContain('rgba(0, 0, 0, 0)')
  expect(iconColors.size).toBe(3)
  await expect.poll(() => attendancePosts).toBe(3)
  await expect(absent).toBeFocused()
  await expect(absent).toHaveCSS('outline-style', 'solid')
  const addNote = page.getByRole('button', { name: 'Add note for ليلى', exact: true })
  await expect(addNote).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)')
  await expect(addNote).toHaveCSS('border-top-width', '0px')
  const dialog = page.getByRole('dialog', { name: 'Student note for ليلى' })
  await addNote.click()
  await expect(dialog.locator('time')).toHaveAttribute('datetime', sessionDate)
  await expect(dialog.getByLabel('Note', { exact: true })).toBeFocused()
  await dialog.getByLabel('Note', { exact: true }).fill('Cancelled note')
  await page.keyboard.press('Escape')
  await expect(dialog).toBeHidden()
  await expect(addNote).toBeFocused()
  await addNote.click()
  await expect(dialog.getByLabel('Note', { exact: true })).toHaveValue('')
  await dialog.getByLabel('Tags, separated by commas').fill('مراجعة, قراءة')
  await dialog.getByLabel('Note', { exact: true }).fill('راجعت السورة اليوم')
  await dialog.getByRole('button', { name: 'Save note' }).click()
  await expect(dialog).toBeHidden()
  await expect(
    page.getByLabel('Attendance for ليلى').getByRole('button', { pressed: true }),
  ).toHaveAccessibleName('absent')
  await addNote.click()
  await expect(dialog.getByText('راجعت السورة اليوم')).toBeVisible()
  await expect(dialog.getByText('Review the last two pages together next session.')).toHaveCount(0)
  if (testInfo.project.name === 'chromium') {
    await mkdir('.artifacts/verification', { recursive: true })
    await page.screenshot({ path: '.artifacts/verification/notebook.png', fullPage: true })
  }
  await page.keyboard.press('Escape')
  expect(attendancePosts).toBe(3)
  await expect(page.getByRole('button', { name: /Save attendance/ })).toHaveCount(0)
  await page.getByRole('button', { name: 'Summary', exact: true }).click()
  const summaryRow = page.getByRole('row', { name: /ليلى/ })
  await expect(summaryRow.locator('td[data-label="absent"]')).toHaveText('1')
  await page.getByRole('button', { name: 'Attendance', exact: true }).click()
  await page.getByLabel('Session date').fill('2001-01-01')
  await addNote.click()
  await expect(dialog.getByText('راجعت السورة اليوم')).toHaveCount(0)
  await dialog.getByLabel('Note', { exact: true }).fill('Earlier session note')
  await dialog.getByRole('button', { name: 'Save note' }).click()
  await expect(dialog).toBeHidden()
  await addNote.click()
  await expect(dialog.getByText('Earlier session note')).toBeVisible()
  await page.keyboard.press('Escape')
  await openNotebookHistory(page)
  const note = page.locator('li').filter({ hasText: 'راجعت السورة اليوم' })
  await clickRecordAction(note, 'Undo')
  await page.getByLabel('Reason').fill('تصحيح')
  await page.getByRole('button', { name: 'Append correction' }).click()
  await expect(note).toContainText('Undone')
  await expect(page.getByText('Correction: تصحيح')).toBeVisible()
  await page.getByRole('button', { name: 'Attendance', exact: true }).click()
  await page.getByLabel('Session date').fill(sessionDate)
  await addNote.click()
  await expect(dialog.getByText('راجعت السورة اليوم')).toHaveCount(0)
  await page.reload()
  await addNote.click()
  await expect(dialog.getByText('راجعت السورة اليوم')).toHaveCount(0)
  await page.keyboard.press('Escape')
  await expect(
    page.getByLabel('Attendance for ليلى').getByRole('button', { pressed: true }),
  ).toHaveAccessibleName('absent')
  expect(external).toEqual([])
})

test('pressing saved attendance again clears the record after sync and reload', async ({
  context,
  hasTouch,
  page,
}) => {
  await mockSyntheticNotebook(page, {
    names: ['Lina'],
    role: hasTouch ? 'teacher' : 'administrator',
  })
  await page.goto(`/?page=app&view=notebook&tab=attendance&course=${courseId}`)
  await expect(
    page.getByLabel('Attendance for Lina').getByRole('button', { pressed: true }),
  ).toHaveAccessibleName('present')
  if (hasTouch) await clickAccountAction(page, 'العربية')

  const marks = page.getByLabel(hasTouch ? 'الحضور للطالب Lina' : 'Attendance for Lina')
  const absent = marks.getByRole('button', { name: hasTouch ? 'غائب' : 'absent', exact: true })
  const selected = marks.getByRole('button', { pressed: true })
  const acknowledged = (status: 'absent' | null) =>
    page.waitForResponse((response) => {
      if (!response.ok() || !response.url().endsWith('/api/attendance/sync')) return false
      const body = offlineAttendanceSyncSchema.parse(response.request().postDataJSON())
      return body.records[0]?.event.payload.status === status
    })
  const activateAbsent = () => (hasTouch ? absent.tap() : absent.click())
  const held = Promise.withResolvers<void>()
  if (!hasTouch) {
    await page.route('**/api/attendance/sync', async (route) => {
      await held.promise
      await route.fallback()
    })
  }
  const replacementAck = acknowledged('absent')
  const clearAck = acknowledged(null)
  try {
    await activateAbsent()
    await expect(absent).toHaveAttribute('aria-pressed', 'true')
    if (hasTouch) {
      await replacementAck
      await page.evaluate(() => navigator.serviceWorker.ready)
      await page.reload()
      await expect(absent).toHaveAttribute('aria-pressed', 'true')
      await expect
        .poll(() => page.evaluate(() => Boolean(navigator.serviceWorker.controller)))
        .toBe(true)
      await context.setOffline(true)
      await expect(page.getByText('غير متصل', { exact: true })).toBeVisible()
      await activateAbsent()
      await expect(page.getByText('حُفظت على هذا الجهاز. بانتظار المزامنة.')).toBeVisible()
      await page.reload()
      await expect(marks).toBeVisible()
      await expect(selected).toHaveCount(0)
      await context.setOffline(false)
    } else {
      await absent.press('Space')
      await expect(absent).toHaveAttribute('aria-pressed', 'false')
      await expect(absent).toBeFocused()
    }
  } finally {
    held.resolve()
  }
  const [replacementResponse, clearResponse] = await Promise.all([replacementAck, clearAck])
  const replacement = offlineAttendanceSyncSchema.parse(
    replacementResponse.request().postDataJSON(),
  ).records[0]
  const clear = offlineAttendanceSyncSchema.parse(clearResponse.request().postDataJSON()).records[0]
  expect(clear).toMatchObject({
    expectedEventId: replacement?.event.id,
    event: { payload: { status: null } },
  })
  expect(await clearResponse.json()).toMatchObject({ outcomes: [{ kind: 'saved' }] })
  await expect(selected).toHaveCount(0)
  await page.reload()
  await expect(marks).toBeVisible()
  await expect(selected).toHaveCount(0)
})

test('individual marks persist across student navigation and can be corrected', async ({
  page,
}) => {
  await mockSyntheticNotebook(page, { names: ['أحمد', 'Omar', 'Lina'] })
  await page.goto('/?page=app')
  await page.getByRole('button', { name: /Continue attendance.*2/i }).click()
  const roster = page.getByRole('region', { name: 'Attendance list' })
  const unmarked = page.getByRole('button', { name: /Show unmarked/i })
  if ((await unmarked.getAttribute('aria-pressed')) !== 'true') await unmarked.click()
  await expect(roster.getByLabel('Attendance for أحمد')).toHaveCount(0)
  await unmarked.click()
  const omar = roster.getByRole('button', { name: 'Omar', exact: true })
  await page
    .getByLabel('Attendance for Omar')
    .getByRole('button', { name: 'excused', exact: true })
    .click()
  await omar.click()
  await expect(page.getByRole('heading', { name: 'Student information' })).toBeVisible()
  await page.goBack()
  await expect(omar).toBeFocused()
  await expect(
    page.getByLabel('Attendance for Omar').getByRole('button', { pressed: true }),
  ).toHaveAccessibleName('excused')
  await expect(page.getByRole('button', { name: /Mark .* unmarked present/i })).toHaveCount(0)
  const lina = roster.getByRole('row').filter({ has: page.getByLabel('Attendance for Lina') })
  await lina.getByRole('button', { name: 'present', exact: true }).click()
  await expect(
    page.getByLabel('Attendance for Lina').getByRole('button', { pressed: true }),
  ).toHaveAccessibleName('present')
  await expect(
    page.getByLabel('Attendance for Omar').getByRole('button', { pressed: true }),
  ).toHaveAccessibleName('excused')
  await expect(
    page.getByLabel('Attendance for أحمد').getByRole('button', { pressed: true }),
  ).toHaveAccessibleName('present')
  await expect(page.getByRole('button', { name: /Save attendance/ })).toHaveCount(0)
  await expect(page.getByText('Changes accepted by Wartaq.')).toBeVisible()
  await lina.getByRole('button', { name: 'Correct this mark: Lina', exact: true }).click()
  await page.getByLabel('Reason', { exact: true }).fill('Wrong student selected')
  await page.getByRole('button', { name: 'Append correction' }).click()
  await expect(lina.getByRole('group').getByRole('button', { pressed: true })).toHaveCount(0)
  await expect(
    page.getByLabel('Attendance for Omar').getByRole('button', { pressed: true }),
  ).toHaveAccessibleName('excused')
})

test('student search finds a normalized Arabic family name', async ({ page }) => {
  await mockSyntheticNotebook(page, { names: ['أحمد', 'Omar', 'سلمى إبراهيم'] })
  await page.goto('/?page=app')
  await page.getByRole('button', { name: 'Students', exact: true }).click()
  await clickAccountAction(page, 'العربية')
  const search = page.getByRole('searchbox', { name: 'ابحث عن طالب' })
  const rows = page.locator('.roster-students-table tbody tr')
  const matchingStudent = rows.filter({ hasText: 'سلمى إبراهيم' })
  const group = page.getByRole('combobox', { name: 'المجموعة', exact: true })
  for (const term of ['ابرا', 'ابراهم']) {
    await search.fill(term)
    await expect(matchingStudent).toBeVisible()
  }
  await group.selectOption('الحلقة الأولى')
  await expect(rows).toHaveCount(0)
  await group.selectOption('')
  await expect(matchingStudent).toBeVisible()
  await search.fill('unrelated')
  await expect(rows).toHaveCount(0)
  await search.clear()
  await expect(rows).toHaveCount(3)
})

test('a signed-in view copies its own address without stray parameters', async ({
  page,
  context,
}) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  await page.goto('/?page=app&token=secret')
  await expect(page.getByRole('button', { name: 'Copy link', exact: true })).toHaveCount(0)
  await mockSyntheticNotebook(page)
  const view = `/?page=app&view=notebook&tab=attendance&course=${courseId}`
  await page.goto(`${view}&token=secret`)
  const expected = new URL(view, page.url()).href
  const copy = page.locator('.copy-view-link > button')
  const manual = page.locator('.copy-view-fallback input')
  await expect(copy).toHaveAccessibleName('Copy link')
  await copy.click()
  await expect(copy).toHaveAccessibleName('Link copied')
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(expected)

  await page.evaluate(() => {
    Object.defineProperty(navigator.clipboard, 'writeText', {
      value: () => Promise.reject(new DOMException('Clipboard denied', 'NotAllowedError')),
    })
  })
  await copy.click()
  await expect(manual).toHaveValue(expected)
  await expect(manual).toBeFocused()
  await page.goto('/')
  await expect(page.getByRole('button', { name: 'Copy link', exact: true })).toHaveCount(0)
})
