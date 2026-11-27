import { mockSyntheticNotebook, openNotebookHistory } from './syntheticNotebook'
import { clickRecordAction } from './recordActions'
import { clickAccountAction } from './profileActions'
import { expect, test } from '@playwright/test'
import { mkdir } from 'node:fs/promises'

test('dashboard fits the session controls and add-student icon in both directions', async ({
  page,
}, testInfo) => {
  await mockSyntheticNotebook(page)
  await page.goto('/?page=app')
  await mkdir('.artifacts/verification', { recursive: true })
  for (const language of ['en', 'ar']) {
    const ar = language === 'ar'
    if (ar) await clickAccountAction(page, 'العربية')
    const controls = page.locator('.dashboard-actions')
    const dateInput = controls.locator('input[type="date"]')
    const status = controls.locator('.dashboard-status')
    const attendance = controls.getByRole('button', {
      name: ar ? /متابعة الحضور/ : /Continue attendance/,
    })
    const add = controls.getByRole('button', {
      name: ar ? 'إضافة طالب' : 'Add student',
      exact: true,
    })
    await expect(add).toHaveText('')
    await expect(add.locator('svg')).toBeVisible()
    for (const width of testInfo.project.name === 'mobile' ? [320, 360] : [1280]) {
      await page.setViewportSize({ width, height: 844 })
      const [dateBox, statusBox, attendanceBox, addBox] = await Promise.all(
        [dateInput, status, attendance, add].map((control) => control.boundingBox()),
      )
      if (!dateBox || !statusBox || !attendanceBox || !addBox) {
        throw new Error('Dashboard controls must be visible')
      }
      expect(statusBox.y).toBeLessThan(dateBox.y + dateBox.height)
      expect(statusBox.y + statusBox.height).toBeGreaterThan(dateBox.y)
      expect(ar ? statusBox.x < dateBox.x : statusBox.x > dateBox.x).toBe(true)
      expect(addBox.width).toBe(44)
      expect(addBox.height).toBe(44)
      expect(addBox.y).toBeLessThan(attendanceBox.y + attendanceBox.height)
      expect(addBox.y + addBox.height).toBeGreaterThan(attendanceBox.y)
      expect(attendanceBox.y < dateBox.y + dateBox.height).toBe(width === 1280)
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      )
      await controls.screenshot({
        path: `.artifacts/verification/dashboard-${width}-${language}.png`,
      })
    }
    await add.click()
    await expect(page).toHaveURL(/view=add-student/)
    await page.goBack()
  }
})

test('dashboard charts and alphabetic attendance reflect immediate marks', async ({
  page,
}, testInfo) => {
  await mockSyntheticNotebook(page)
  await page.goto('/?page=app')
  await expect(page.getByRole('button', { name: /Continue attendance/i })).toBeVisible()
  await expect(page.getByText('No recorded absences to follow up in this window.')).toBeVisible()
  await expect(page.getByText('1 marked, 1 unmarked')).toBeVisible()
  await clickAccountAction(page, 'العربية')
  await expect(page.getByRole('button', { name: /متابعة/ })).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
    true,
  )
  await clickAccountAction(page, 'English')
  await page.getByRole('button', { name: /Continue attendance/i }).click()
  await expect(page.locator('details.attendance-group')).toHaveCount(0)
  const names = await page
    .getByRole('region', { name: 'Attendance list' })
    .locator('tbody tr td:first-child')
    .allTextContents()
  expect(names).toEqual([...names].sort((a, b) => a.localeCompare(b, 'en')))
  await expect(
    page
      .getByRole('region', { name: 'Attendance list' })
      .locator('thead th > button.table-sort, thead th:not(:has(button.table-sort))'),
  ).toHaveText(['Student', 'Level', 'Teacher', 'Attendance'])
  const mark = page.getByRole('group', { name: 'Attendance for Omar' })
  const absent = mark.getByRole('button', { name: 'absent', exact: true })
  await expect(mark.getByRole('button')).toHaveCount(3)
  for (const name of ['present', 'absent', 'excused']) {
    await expect(mark.getByRole('button', { name, exact: true })).toBeVisible()
  }
  await expect(mark.getByRole('button', { pressed: true })).toHaveCount(0)
  await expect(page.getByRole('button', { name: /Save attendance/ })).toHaveCount(0)
  if (testInfo.project.name === 'mobile') {
    await mkdir('.artifacts/verification', { recursive: true })
    for (const language of ['en', 'ar']) {
      if (language === 'ar') await clickAccountAction(page, 'العربية')
      for (const width of [320, 390, 430, 844]) {
        await page.setViewportSize({ width, height: width === 844 ? 390 : 844 })
        const table = page.getByRole('region', {
          name: language === 'en' ? 'Attendance list' : 'قائمة الحضور',
        })
        expect(await table.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(
          true,
        )
        await expect(table.locator('td[data-label]').first()).toBeVisible()
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        )
        await page.screenshot({
          path: `.artifacts/verification/attendance-${width}-${language}.png`,
          fullPage: true,
        })
      }
    }
    await clickAccountAction(page, 'English')
  }
  await absent.press('Space')
  await expect(mark).toHaveCount(0)
  await expect(page.getByText('Changes accepted by Wartaq.')).toBeVisible()
  await page.getByRole('button', { name: 'Dashboard', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Omar', exact: true })).toBeVisible()
  await expect(page.getByText('2 marked, 0 unmarked')).toBeVisible()
  await openNotebookHistory(page)
  const record = page.locator('li').filter({ hasText: 'Omar' }).filter({ hasText: 'absent' })
  await clickRecordAction(record, 'Record details')
  await expect(record.locator('pre')).toContainText('"enrollmentId"')
  for (const language of ['en', 'ar']) {
    if (language === 'ar') await clickAccountAction(page, 'العربية')
    for (const width of [320, 390]) {
      await page.setViewportSize({ width, height: 844 })
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      )
    }
  }
  await clickAccountAction(page, 'English')
  await clickRecordAction(record, 'Undo')
  await page.getByLabel('Reason').fill('Correction')
  await page.getByRole('button', { name: 'Append correction' }).click()
  await page.getByRole('button', { name: 'Dashboard', exact: true }).click()
  await expect(page.getByText('No recorded absences to follow up in this window.')).toBeVisible()
  await expect(page.getByText('1 marked, 1 unmarked')).toBeVisible()
})

test('follow-up uses recorded absences through the selected date', async ({ page }) => {
  await mockSyntheticNotebook(page, { role: 'teacher' })
  await page.goto('/?page=app')
  const dateInput = page.getByLabel('Session date')
  const firstDate = await dateInput.inputValue()
  const nextDate = new Date(`${firstDate}T00:00:00Z`)
  const daysUntilWednesday = (3 - nextDate.getUTCDay() + 7) % 7 || 7
  nextDate.setUTCDate(nextDate.getUTCDate() + daysUntilWednesday)
  const secondDate = nextDate.toISOString().slice(0, 10)

  await page.getByRole('button', { name: /Continue attendance/i }).click()
  await page
    .getByLabel('Attendance for Omar')
    .getByRole('button', { name: 'absent', exact: true })
    .click()
  await page.getByLabel('Session date').fill(secondDate)
  await page
    .getByLabel('Attendance for Omar')
    .getByRole('button', { name: 'excused', exact: true })
    .click()
  const followingDate = new Date(nextDate)
  for (let index = 0; index < 4; index += 1) {
    followingDate.setUTCDate(followingDate.getUTCDate() + 1)
    await page.getByLabel('Session date').fill(followingDate.toISOString().slice(0, 10))
    await page
      .getByLabel('Attendance for Omar')
      .getByRole('button', { name: 'present', exact: true })
      .click()
  }
  await page.getByRole('button', { name: 'Dashboard', exact: true }).click()
  const strip = page.getByRole('list', { name: 'Recent attendance for Omar' })
  const cards = strip.getByRole('listitem')
  await expect(cards).toHaveCount(6)
  for (const [index, [status, color]] of [
    ['absent', 'rgb(220, 38, 38)'],
    ['excused', 'rgb(161, 98, 7)'],
    ['present', 'rgb(22, 128, 60)'],
  ].entries()) {
    const card = cards.nth(index)
    await expect(card).toHaveCSS('border-block-start-color', color)
    await expect(card.locator('span')).toHaveCSS('width', '1px')
    expect(await card.ariaSnapshot()).toContain(status)
  }
  await expect(page.getByText('1 absence', { exact: true })).toBeVisible()

  const dateLabel = page.locator(`.dashboard-strip time[datetime="${secondDate}"]`)
  const day = nextDate.getUTCDate()
  const month = nextDate.getUTCMonth() + 1
  await expect(dateLabel).toHaveText(new RegExp(`^Wednesday,? 0?${day}/0?${month}$`))
  for (const language of ['en', 'ar']) {
    if (language === 'ar') {
      await clickAccountAction(page, 'العربية')
      await expect(dateLabel).toHaveText(
        /^الأربعاء، \p{Decimal_Number}{1,2}\u200f?\/\p{Decimal_Number}{1,2}$/u,
      )
    }
    await page.evaluate(() => document.fonts.ready)
    for (const width of [320, 360, 390, 430, 662, 1280]) {
      await page.setViewportSize({ width, height: 844 })
      const label = `${language} at ${width}px`
      const layout = await page.locator('.dashboard-strip time').evaluateAll((dates) => {
        const range = document.createRange()
        return {
          rows: new Set(dates.map((date) => date.parentElement?.getBoundingClientRect().top)).size,
          lines: dates.map((date) => {
            const cardBounds = date.parentElement?.getBoundingClientRect()
            range.selectNodeContents(date)
            const rects = Array.from(range.getClientRects())
            return {
              rows: new Set(rects.map((rect) => rect.top)).size,
              fits:
                !!cardBounds &&
                rects.every(
                  (rect) => rect.left >= cardBounds.left && rect.right <= cardBounds.right,
                ),
            }
          }),
          viewportFits: document.documentElement.scrollWidth <= innerWidth,
        }
      })
      expect(layout.lines, label).toEqual(
        Array.from({ length: 6 }, () => ({ rows: 1, fits: true })),
      )
      expect(layout.viewportFits, label).toBe(true)
      if (width === 1280 || (width === 662 && language === 'ar')) {
        expect(layout.rows, label).toBe(1)
      }
      if (width === 320) expect(layout.rows, label).toBeGreaterThan(1)
    }
  }
  await clickAccountAction(page, 'English')
  await page.getByLabel('Session date').fill(firstDate)
  await expect(strip.locator('li')).toHaveCount(1)
  await page.getByRole('button', { name: 'Omar', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Student information' })).toBeVisible()
})
