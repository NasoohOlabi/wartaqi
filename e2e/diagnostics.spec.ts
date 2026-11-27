import { expect, test } from '@playwright/test'
import { mockSyntheticNotebook } from './syntheticNotebook'

for (const language of ['en', 'ar'] as const) {
  for (const width of [320, 1280]) {
    test(`retired diagnostics bookmark opens Settings without telemetry (${language}, ${width})`, async ({
      page,
    }) => {
      await page.setViewportSize({ width, height: 800 })
      await mockSyntheticNotebook(page)
      await page.addInitScript(
        (value) => localStorage.setItem('halaqa-language-preference', value),
        language,
      )
      const requests: string[] = []
      page.on('request', (request) => {
        const path = new URL(request.url()).pathname
        if (path.startsWith('/api/diagnostics')) requests.push(path)
      })
      await page.goto('/?page=app&view=diagnostics&account=retired-filter')
      const accounts = page.getByRole('link', {
        name: language === 'ar' ? 'الحسابات' : 'Accounts',
        exact: true,
      })
      await expect(accounts).toHaveAttribute('aria-current', 'page')
      await expect(
        page.getByRole('link', { name: language === 'ar' ? 'تيليغرام' : 'Telegram', exact: true }),
      ).toBeVisible()
      await expect(page.getByRole('link', { name: /Diagnostics|التشخيص/ })).toHaveCount(0)
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
      ).toBe(true)
      expect(requests).toEqual([])
    })
  }
}
