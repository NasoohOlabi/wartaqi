import { expect, test } from '@playwright/test'
import { mockSyntheticNotebook } from './syntheticNotebook'

test('attendance sync sends another edit while a post-save read is slow', async ({ page }) => {
  await mockSyntheticNotebook(page, { names: ['First student', 'Second student'] })
  await page.goto('/?page=app&tab=attendance')
  const first = page.getByRole('group', { name: 'Attendance for First student' })
  const second = page.getByRole('group', { name: 'Attendance for Second student' })
  await expect(first).toBeVisible()

  const read = Promise.withResolvers<void>()
  const release = Promise.withResolvers<void>()
  let writes = 0
  await page.route('**/api/attendance/sync', (route) => {
    writes += 1
    return route.fallback()
  })
  await page.route('**/api/notebook', async (route) => {
    if (writes > 0) {
      read.resolve()
      await release.promise
    }
    return route.fallback()
  })

  try {
    await first.getByRole('button', { name: 'absent' }).click()
    await read.promise
    const secondSave = page.waitForResponse('**/api/attendance/sync', { timeout: 3_000 })
    await second.getByRole('button', { name: 'excused' }).click()
    expect((await secondSave).ok()).toBe(true)
    expect(writes).toBe(2)
  } finally {
    release.resolve()
  }

  await page.reload()
  await expect(first.getByRole('button', { name: 'absent' })).toHaveAttribute(
    'aria-pressed',
    'true',
  )
  await expect(second.getByRole('button', { name: 'excused' })).toHaveAttribute(
    'aria-pressed',
    'true',
  )
})

test('administrator reconnects and syncs attendance from Settings', async ({ context, page }) => {
  await mockSyntheticNotebook(page)
  let connected = true
  await page.route('**/api/**', (route) =>
    connected ? route.fallback() : route.abort('internetdisconnected'),
  )
  let notebookReads = 0
  page.on('request', (request) => {
    if (request.url().endsWith('/api/notebook')) notebookReads++
  })
  await page.goto('/?page=app&view=settings')
  await expect(page.getByRole('heading', { name: 'Accounts', exact: true })).toBeVisible()
  expect(notebookReads).toBe(0)
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Attendance', exact: true })
    .click()
  const attendance = page.getByRole('group', { name: / Omar$/ })
  await expect(attendance).toBeVisible()
  await expect
    .poll(() => page.evaluate(() => Boolean(localStorage.getItem('halaqa-offline-access'))))
    .toBe(true)

  connected = false
  await context.setOffline(true)
  await expect(page.getByText('Offline', { exact: true })).toBeVisible()
  await attendance.getByRole('button', { name: 'absent', exact: true }).click()
  await expect(
    page.getByText('Saved on this device. Waiting to sync.', { exact: true }),
  ).toBeVisible()
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Settings', exact: true })
    .click()
  const settings = page.getByRole('heading', { name: 'Settings', exact: true })
  await expect(settings).toBeVisible()
  const waiting = page.getByText('Changes saved on this device are waiting to sync.')
  await expect(waiting).toBeVisible()
  await page.getByRole('button', { name: 'My changes', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'My changes', exact: true })).toBeVisible()
  await expect(page.getByText('Omar', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Back', exact: true }).click()
  await expect(settings).toBeVisible()
  await expect(waiting).toBeVisible()

  const synced = page.waitForResponse(
    (response) => response.url().endsWith('/api/attendance/sync') && response.ok(),
  )
  connected = true
  await context.setOffline(false)
  await synced
  await expect(waiting).toHaveCount(0)
  await expect(settings).toBeVisible()
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Attendance', exact: true })
    .click()
  await expect(attendance.getByRole('button', { name: 'absent', exact: true })).toHaveAttribute(
    'aria-pressed',
    'true',
  )
})
