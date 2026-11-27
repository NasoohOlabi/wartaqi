import { expect, test } from '@playwright/test'
import { mockSyntheticNotebook } from './syntheticNotebook'

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
  await page.getByRole('button', { name: 'Save attendance (1)' }).click()
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
