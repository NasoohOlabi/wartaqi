import { expect, test } from '@playwright/test'
import { diagnosticsBatchSchema } from '../shared/diagnostics'
import { mockSyntheticNotebook } from './syntheticNotebook'
import { clickAccountAction } from './profileActions'

for (const language of ['en', 'ar'] as const) {
  test(`administrator diagnostics journey (${language})`, async ({ page }) => {
    await page.setViewportSize(
      language === 'ar' ? { width: 320, height: 800 } : { width: 1280, height: 800 },
    )
    await mockSyntheticNotebook(page, { names: ['Private Record'] })
    const batches: string[] = []
    const journeyId = '20000000-0000-4000-8000-000000000001'
    const actorId = 'synthetic-actor'
    await page.route('**/api/diagnostics**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname === '/api/diagnostics') {
        const body = route.request().postData() ?? ''
        diagnosticsBatchSchema.parse(JSON.parse(body))
        batches.push(body)
        await route.fulfill({ json: { ok: true } })
      } else if (url.pathname === '/api/diagnostics/journeys') {
        await route.fulfill({
          json: {
            items: [
              {
                journeyId,
                actorId,
                actorName: 'Synthetic Journey',
                role: 'teacher',
                lastSeenAt: '2026-11-23T12:00:00.000Z',
                eventCount: 1,
              },
            ],
            nextCursor: null,
          },
        })
      } else {
        expect(url.pathname).toBe('/api/diagnostics/events')
        expect(url.searchParams.get('journey')).toBe(journeyId)
        expect(url.searchParams.get('actor')).toBe(actorId)
        await route.fulfill({
          json: {
            items: [
              {
                event: {
                  id: '30000000-0000-4000-8000-000000000001',
                  journeyId,
                  step: 1,
                  screen: 'dashboard',
                  kind: 'vital',
                  metric: 'LCP',
                  value: 123,
                },
                receivedAt: '2026-11-23T12:00:00.000Z',
              },
            ],
            nextCursor: null,
          },
        })
      }
    })
    await page.goto('/?page=app&privateQuery=secret-value')
    await expect(page.getByRole('heading', { name: 'Attendance & notes' })).toBeVisible()
    if (language === 'ar') await clickAccountAction(page, 'العربية')
    await page.getByLabel(language === 'ar' ? 'تغيير الدورة' : 'Change course').click()
    await page.getByRole('button', { name: 'Synthetic course', exact: true }).click()
    await clickAccountAction(page, language === 'ar' ? 'الإعدادات' : 'Settings')
    await page
      .getByRole('link', { name: language === 'ar' ? 'التشخيص' : 'Diagnostics', exact: true })
      .click()
    await expect(page.getByText('Synthetic Journey')).toBeVisible()
    await page
      .getByRole('button', { name: language === 'ar' ? 'عرض التسلسل' : 'View timeline' })
      .click()
    await expect(page.getByText(/LCP (123|١٢٣) ms/)).toBeVisible()
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    )
    const received = () =>
      batches.flatMap((body) => diagnosticsBatchSchema.parse(JSON.parse(body)).events)
    await expect
      .poll(
        () =>
          received().some((event) => event.kind === 'action' && event.action === 'course-switch'),
        { timeout: 20000 },
      )
      .toBe(true)
    const events = received()
    expect(events.some((event) => event.kind === 'screen' && event.screen === 'dashboard')).toBe(
      true,
    )
    expect(batches.join()).not.toMatch(/secret-value|privateQuery|Private Record/)
  })
}
