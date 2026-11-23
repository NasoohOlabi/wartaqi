import { expect, test } from '@playwright/test'
import { replayBatchSchema, replayPath, type ReplayFrame } from '../shared/sessionReplay'
import { clickAccountAction } from './profileActions'
import { mockSyntheticNotebook } from './syntheticNotebook'

for (const language of ['en', 'ar'] as const) {
  test(`session replay shows a compact Courses activity timeline (${language})`, async ({
    page,
  }, testInfo) => {
    const ar = language === 'ar'
    const courses = ar ? 'الدورات' : 'Courses'
    await page.setViewportSize({ width: ar ? 320 : 1280, height: 800 })
    await page.addInitScript((locale) => {
      localStorage.setItem('halaqa-language-preference', locale)
      localStorage.setItem('halaqa-theme', 'dark')
      const enable = () => {
        document.documentElement.dataset.sessionReplay = 'test'
      }
      if (document.documentElement) enable()
      else document.addEventListener('DOMContentLoaded', enable, { once: true })
    }, language)
    await mockSyntheticNotebook(page, { names: ['Private Student Example'] })
    const frames: ReplayFrame[] = []
    let journeyId = ''
    const actorId = 'synthetic-replay-actor'
    await page.route('**/api/diagnostics/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname === replayPath && route.request().method() === 'POST') {
        const body = route.request().postData() ?? ''
        expect(body).not.toMatch(/hidden-search-example|hidden-form-example/)
        const batch = replayBatchSchema.parse(JSON.parse(body))
        expect(JSON.stringify(batch.frames.filter((frame) => !frame.attendance))).not.toContain(
          'Private Student Example',
        )
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
      return route.fulfill({
        json: { items: frames.filter((frame) => !frame.attendance).slice(0, 20), nextAfter: null },
      })
    })
    await page.goto('/?page=app&q=hidden-search-example')
    await expect(
      page.getByRole('heading', { name: ar ? 'الحضور والملاحظات' : 'Attendance & notes' }),
    ).toBeVisible()
    await expect.poll(() => frames.some((frame) => frame.kind === 'view')).toBe(true)
    await page.evaluate(() => {
      const paragraph = document.createElement('p')
      paragraph.textContent = 'Private Student Example'
      const input = document.createElement('input')
      input.value = 'hidden-form-example'
      document.querySelector('main')?.prepend(paragraph, input)
    })
    await page.getByRole('button', { name: courses, exact: true }).click()
    await expect(page.getByRole('heading', { name: courses, exact: true })).toBeVisible()
    const click = () =>
      frames.find((frame) => frame.kind === 'click' && frame.screen === 'dashboard')
    await expect.poll(click).toBeDefined()
    expect(click()?.boxes.some((box) => box.kind === 'field')).toBe(true)
    await expect.poll(() => frames.some((frame) => frame.screen === 'courses')).toBe(true)
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
    await expect(player.getByRole('heading', { name: ar ? 'النشاط' : 'Activity' })).toBeVisible()
    await expect(player.locator('canvas')).toHaveCount(0)
    await expect(
      player.getByText(
        ar ? 'سُجّل النشاط فقط لهذه الشاشة.' : 'Only activity was recorded for this screen.',
      ),
    ).toBeVisible()
    const selected = player.locator('button[aria-current="true"]')
    await player
      .getByRole('button', { name: language === 'ar' ? 'النقرة التالية' : 'Next click' })
      .click()
    await expect(selected).toContainText(courses)
    const courseView = player
      .getByRole('button', {
        name: ar ? /عرض.*الدورات|الدورات.*عرض/ : /Viewed.*Courses|Courses.*Viewed/,
      })
      .first()
    await courseView.click()
    await expect(courseView).toHaveAttribute('aria-current', 'true')
    await expect(player).toHaveCSS('direction', ar ? 'rtl' : 'ltr')
    expect((await player.boundingBox())?.height).toBeLessThan(800)
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    )
    await player.screenshot({ path: testInfo.outputPath(`replay-${language}.png`) })
  })
}
