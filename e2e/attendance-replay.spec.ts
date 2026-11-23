import { expect, test } from '@playwright/test'
import { replayBatchSchema, replayPath, type ReplayFrame } from '../shared/sessionReplay'
import { clickAccountAction } from './profileActions'
import { mockSyntheticNotebook } from './syntheticNotebook'

for (const language of ['en', 'ar'] as const) {
  test(`attendance replay renders the recorded roster, mark and save (${language})`, async ({
    page,
  }) => {
    const ar = language === 'ar'
    await page.setViewportSize({ width: ar ? 320 : 1280, height: 800 })
    await page.addInitScript((locale) => {
      localStorage.setItem('halaqa-language-preference', locale)
      const enable = () => {
        document.documentElement.dataset.sessionReplay = 'test'
      }
      if (document.documentElement) enable()
      else document.addEventListener('DOMContentLoaded', enable, { once: true })
    }, language)
    const student = 'Synthetic Replay Student'
    await mockSyntheticNotebook(page, { names: ['Synthetic Existing Student', student] })
    let journey = ''
    const actor = 'synthetic-replay-actor'
    const frames: ReplayFrame[] = []
    const appCalls: string[] = []
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname
      if (path === '/api/events' || path === '/api/notebook')
        appCalls.push(`${request.method()} ${path}`)
    })
    await page.route('**/api/diagnostics/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname === replayPath && route.request().method() === 'POST') {
        const body = route.request().postData() ?? ''
        expect(body).not.toMatch(/Review the last two pages|synthetic@example.test/)
        const batch = replayBatchSchema.parse(JSON.parse(body))
        journey = batch.journeyId
        frames.push(...batch.frames.filter((frame) => frame.attendance))
        return route.fulfill({ json: { ok: true } })
      }
      if (url.pathname === '/api/diagnostics/journeys')
        return route.fulfill({
          json: {
            items: [
              {
                journeyId: journey,
                actorId: actor,
                actorName: 'Synthetic Replay Actor',
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
      expect(url.searchParams.get('journey')).toBe(journey)
      expect(url.searchParams.get('actor')).toBe(actor)
      return route.fulfill({ json: { items: frames.slice(0, 20), nextAfter: null } })
    })

    await page.goto('/?page=app&tab=attendance&course=40000000-0000-4000-8000-000000000001')
    const present = ar ? 'حاضر' : 'present'
    await expect
      .poll(() =>
        frames.some((frame) => frame.attendance?.rows.some((row) => row.name === student)),
      )
      .toBe(true)
    const row = page.locator('tr').filter({ hasText: student })
    await row.getByRole('button', { name: present, exact: true }).click()
    await expect
      .poll(() =>
        frames.some((frame) =>
          frame.attendance?.rows.some((row) => row.name === student && row.draft === 'present'),
        ),
      )
      .toBe(true)
    await page.getByRole('button', { name: ar ? /حفظ الحضور/ : /Save attendance/ }).click()
    await expect
      .poll(() =>
        frames.some((frame) =>
          frame.attendance?.rows.some(
            (row) => row.name === student && row.saved === 'present' && row.draft === null,
          ),
        ),
      )
      .toBe(true)
    expect(
      frames.some((frame) => frame.kind === 'click' && frame.attendanceAction?.kind === 'mark'),
    ).toBe(true)

    await clickAccountAction(page, ar ? 'الإعدادات' : 'Settings')
    await page.getByRole('link', { name: ar ? 'التشخيص' : 'Diagnostics', exact: true }).click()
    await page.getByRole('button', { name: ar ? 'عرض التسلسل' : 'View timeline' }).click()
    await page.getByRole('button', { name: ar ? 'إعادة تشغيل الجلسة' : 'Replay session' }).click()
    const player = page.getByRole('region', { name: ar ? 'إعادة تشغيل الجلسة' : 'Session replay' })
    await expect(player.getByRole('heading', { name: /Synthetic course/ })).toBeVisible()
    const callsBeforePlayback = appCalls.length
    await player.getByRole('button', { name: ar ? 'النقرة التالية' : 'Next click' }).click()
    await expect(player.getByRole('status').filter({ hasText: student })).toBeVisible()
    await expect(player.locator('[data-replay-highlight="true"]')).toContainText(student)
    await player
      .getByRole('slider', { name: ar ? 'موضع التشغيل' : 'Playback position' })
      .press('End')
    const replayStatus = player
      .locator('tr')
      .filter({ hasText: student })
      .getByRole('button', { name: present, exact: true })
    await expect(replayStatus).toHaveAttribute('aria-pressed', 'true')
    await expect(replayStatus).toBeDisabled()
    expect(appCalls).toHaveLength(callsBeforePlayback)
    expect(appCalls.filter((call) => call === 'POST /api/events')).toHaveLength(1)
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    )
  })
}
