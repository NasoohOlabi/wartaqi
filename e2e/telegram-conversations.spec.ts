import { expect, test } from '@playwright/test'
import { mockSyntheticNotebook } from './syntheticNotebook'
import { clickAccountAction } from './profileActions'

for (const language of ['en', 'ar'] as const) {
  for (const width of [320, 1280]) {
    test(`Telegram conversations ${language} ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 800 })
      await mockSyntheticNotebook(page)
      const conversationId = 'a'.repeat(64)
      await page.route('**/api/diagnostics/telegram/**', async (route) => {
        const url = new URL(route.request().url())
        if (url.pathname.endsWith('/conversations')) {
          await route.fulfill({
            json: {
              items: [{ conversationId, lastSeenAt: '2026-11-23T12:00:00.000Z', turnCount: 2 }],
              nextCursor: null,
            },
          })
        } else {
          expect(url.searchParams.get('conversation')).toBe(conversationId)
          const older = url.searchParams.has('before')
          await route.fulfill({
            json: {
              items: [
                {
                  trace: older ? 'oldertrace01' : 'newertrace01',
                  question: older ? 'Older question' : '<img src=x onerror=alert(1)>',
                  reply: older ? 'Older reply' : 'New reply',
                  delivery: older ? 'sent' : 'failed',
                  outcome: older ? 'ok' : 'timeout',
                  promptVersion: 'wartaq-help-v1',
                  model: 'synthetic',
                  elapsedMs: 120,
                  receivedAt: older ? '2026-11-23T11:00:00.000Z' : '2026-11-23T12:00:00.000Z',
                },
              ],
              nextCursor: older ? null : '1',
            },
          })
        }
      })
      await page.goto('/?page=app')
      await expect(page.getByRole('heading', { name: 'Attendance & notes' })).toBeVisible()
      if (language === 'ar') await clickAccountAction(page, 'العربية')
      await clickAccountAction(page, language === 'ar' ? 'الإعدادات' : 'Settings')
      await page
        .getByRole('link', { name: language === 'ar' ? 'تيليغرام' : 'Telegram', exact: true })
        .click()
      await page
        .getByRole('button', { name: language === 'ar' ? 'فتح المحادثة' : 'Open conversation' })
        .click()
      await expect(page.getByText('<img src=x onerror=alert(1)>', { exact: true })).toBeVisible()
      await expect(page.locator('img[src="x"]')).toHaveCount(0)
      await page
        .getByRole('button', {
          name: language === 'ar' ? 'تحميل الرسائل الأقدم' : 'Load older messages',
        })
        .click()
      await expect(page.getByText('Older question')).toBeVisible()
      const messages = await page.locator('article').allTextContents()
      expect(messages[0]).toContain('Older question')
      expect(messages[1]).toContain('New reply')
      expect(await page.locator('main').innerText()).not.toContain(conversationId)
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
      ).toBe(true)
    })
  }
}

test('teacher cannot open Telegram diagnostics', async ({ page }) => {
  await mockSyntheticNotebook(page, { role: 'teacher' })
  let reads = 0
  await page.route('**/api/diagnostics/telegram/**', (route) => {
    reads++
    return route.abort()
  })
  await page.goto('/?page=app&view=telegram')
  await expect(page.getByRole('heading', { name: 'Attendance & notes' })).toBeVisible()
  await expect(page.getByText('Telegram conversations')).toHaveCount(0)
  expect(reads).toBe(0)
})
