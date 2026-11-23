import { expect, test } from '@playwright/test'
import { replayBatchSchema, replayPath, type ReplayFrame } from '../shared/sessionReplay'
import { clickAccountAction } from './profileActions'
import { mockSyntheticNotebook } from './syntheticNotebook'

for (const language of ['en', 'ar'] as const) {
  test(`session replay shows a redacted viewport and the recorded click (${language})`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: language === 'en' ? 1280 : 320, height: 800 })
    await page.addInitScript(() => {
      const enable = () => {
        document.documentElement.dataset.sessionReplay = 'test'
      }
      if (document.documentElement) enable()
      else document.addEventListener('DOMContentLoaded', enable, { once: true })
    })
    await mockSyntheticNotebook(page, { names: ['Private Student Example'] })
    const frames: ReplayFrame[] = []
    let journeyId = ''
    const actorId = 'synthetic-replay-actor'
    await page.route('**/api/diagnostics/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname === replayPath && route.request().method() === 'POST') {
        const body = route.request().postData() ?? ''
        expect(body).not.toMatch(
          /Private Student Example|hidden-search-example|hidden-form-example/,
        )
        const batch = replayBatchSchema.parse(JSON.parse(body))
        journeyId = batch.journeyId
        frames.push(...batch.frames)
        return route.fulfill({ json: { ok: true } })
      }
      if (url.pathname === '/api/diagnostics/journeys')
        return route.fulfill({
          json: {
            items: [
              {
                journeyId,
                actorId,
                actorName: 'Synthetic Replay',
                role: 'administrator',
                lastSeenAt: new Date().toISOString(),
                eventCount: 1,
              },
            ],
            nextCursor: null,
          },
        })
      if (url.pathname === '/api/diagnostics/events')
        return route.fulfill({ json: { items: [], nextCursor: null } })
      expect(url.pathname).toBe(replayPath)
      expect(url.searchParams.get('journey')).toBe(journeyId)
      expect(url.searchParams.get('actor')).toBe(actorId)
      return route.fulfill({ json: { items: frames.slice(0, 20), nextAfter: null } })
    })
    await page.goto('/?page=app&q=hidden-search-example')
    await expect(page.getByRole('heading', { name: 'Attendance & notes' })).toBeVisible()
    await expect.poll(() => frames.some((frame) => frame.kind === 'view')).toBe(true)
    await page.evaluate(() => {
      const paragraph = document.createElement('p')
      paragraph.textContent = 'Private Student Example'
      const input = document.createElement('input')
      input.value = 'hidden-form-example'
      document.querySelector('main')?.prepend(paragraph, input)
    })
    await page.getByRole('button', { name: 'Take attendance' }).click()
    const click = () =>
      frames.find((frame) => frame.kind === 'click' && frame.screen === 'dashboard')
    await expect.poll(click).toBeDefined()
    expect(click()?.boxes.some((box) => box.kind === 'field')).toBe(true)
    if (language === 'ar') await clickAccountAction(page, 'العربية')
    await clickAccountAction(page, language === 'ar' ? 'الإعدادات' : 'Settings')
    await page
      .getByRole('link', { name: language === 'ar' ? 'التشخيص' : 'Diagnostics', exact: true })
      .click()
    await page
      .getByRole('button', { name: language === 'ar' ? 'عرض التسلسل' : 'View timeline' })
      .click()
    await page
      .getByRole('button', { name: language === 'ar' ? 'إعادة تشغيل الجلسة' : 'Replay session' })
      .click()
    const player = page.getByRole('region', {
      name: language === 'ar' ? 'إعادة تشغيل الجلسة' : 'Session replay',
    })
    await expect(player.locator('canvas')).toBeVisible()
    await player
      .getByRole('button', { name: language === 'ar' ? 'النقرة التالية' : 'Next click' })
      .click()
    await expect(player.getByText(/(?:Click|نقرة) \(\d+, \d+\)/)).toBeVisible()
    const recorded = click()
    if (!recorded || recorded.kind !== 'click') throw new Error('Click frame missing')
    const marker = await player.locator('canvas').evaluate((canvas, position) => {
      if (!(canvas instanceof HTMLCanvasElement)) return false
      const pixel = canvas.getContext('2d')?.getImageData(position.x + 12, position.y, 1, 1).data
      return pixel?.[0] === 239 && pixel[1] === 68 && pixel[2] === 68
    }, recorded.click)
    expect(marker).toBe(true)
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    )
  })
}
