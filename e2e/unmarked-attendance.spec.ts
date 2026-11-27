import { expect, test } from '@playwright/test'
import { courseId, mockSyntheticNotebook } from './syntheticNotebook'
import {
  offlineAttendanceResponseSchema,
  offlineAttendanceSyncSchema,
} from '../shared/offlineAttendance'

const names = Array.from(
  { length: 24 },
  (_, index) => `Practice student ${String(index + 1).padStart(2, '0')}`,
)

test('Show unmarked confirms the mark and removes rows without a scroll jump', async ({
  page,
}, testInfo) => {
  const ar = testInfo.project.name === 'mobile'
  await page.addInitScript(
    (language) => {
      localStorage.setItem('halaqa-language-preference', language)
    },
    ar ? 'ar' : 'en',
  )
  await mockSyntheticNotebook(page, { names, role: 'teacher' })
  await page.goto(`/?page=app&tab=attendance&course=${courseId}&unmarked=1`)

  const attendance = (index: number) =>
    page.getByRole('group', {
      name: `${ar ? 'الحضور للطالب' : 'Attendance for'} ${names[index]}`,
      exact: true,
    })
  await expect(attendance(12)).toBeVisible()
  await expect(attendance(0)).toHaveCount(0)

  const present = attendance(12).getByRole('button', { name: ar ? 'حاضر' : 'present' })
  const frames = await present.evaluate(async (button) => {
    if (!(button instanceof HTMLButtonElement)) throw new Error('Missing attendance button')
    const row = button.closest('tr')
    const next = row?.nextElementSibling
    if (!row || !next) throw new Error('Expected two adjacent attendance rows')
    button.scrollIntoView({ block: 'center', behavior: 'instant' })
    button.focus({ preventScroll: true })
    const sample = () => ({
      scroll: window.scrollY,
      nextTop: next.getBoundingClientRect().top,
      retained: row.isConnected,
      pressed: button.getAttribute('aria-pressed') === 'true',
      opacity: Number(getComputedStyle(row).opacity),
    })
    const before = sample()
    const frames = [{ at: 0, ...before }]
    const started = performance.now()
    button.click()
    while (performance.now() - started < 2400) {
      const now = await new Promise<number>((resolve) => requestAnimationFrame(resolve))
      frames.push({ at: now - started, ...sample() })
      if (!row.isConnected) break
    }
    return frames
  })
  const before = frames[0]
  expect(before.scroll).toBeGreaterThan(0)
  const confirmation = frames.find((frame) => frame.at >= 200)
  expect(confirmation).toMatchObject({ retained: true, pressed: true, opacity: 1 })
  expect(confirmation?.nextTop).toBeCloseTo(before.nextTop, 0)
  expect(confirmation?.scroll).toBe(before.scroll)
  expect(frames.at(-1)?.retained).toBe(false)
  expect(frames.some(({ opacity }) => opacity > 0 && opacity < 1)).toBe(true)
  expect(
    frames.slice(1).every((frame, index) => Math.abs(frame.scroll - frames[index].scroll) < 48),
  ).toBe(true)
  expect(
    frames.slice(1).every((frame, index) => Math.abs(frame.nextTop - frames[index].nextTop) < 94),
  ).toBe(true)

  // The final row used to clamp the document scroll when the list shrank in one frame.
  const last = attendance(23)
  const absent = last.getByRole('button', { name: ar ? 'غائب' : 'absent' })
  await absent.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight))
  const bottom = await absent.evaluate(async (button) => {
    if (!(button instanceof HTMLButtonElement)) throw new Error('Missing attendance button')
    const started = performance.now()
    const scroll = window.scrollY
    const frames = [scroll]
    button.click()
    while (performance.now() - started < 2400 && button.isConnected) {
      await new Promise<number>((resolve) => requestAnimationFrame(resolve))
      frames.push(window.scrollY)
    }
    return frames
  })
  await expect(last).toHaveCount(0)
  expect(bottom.at(-1)).toBeLessThan(bottom[0])
  expect(bottom.slice(1).every((scroll, index) => Math.abs(scroll - bottom[index]) < 94)).toBe(true)

  const another = attendance(11).getByRole('button', { name: ar ? 'معذور' : 'excused' })
  await another.click()
  await expect(another).toHaveAttribute('aria-pressed', 'true')
  const cleared = page.waitForResponse((response) => {
    if (new URL(response.url()).pathname !== '/api/attendance/sync') return false
    const submitted = offlineAttendanceSyncSchema.safeParse(response.request().postDataJSON())
    return (
      submitted.success && submitted.data.records.some(({ event }) => event.payload.status === null)
    )
  })
  await another.click()
  await expect(another).toHaveAttribute('aria-pressed', 'false')
  await expect(attendance(11)).toBeVisible()
  const acknowledgement = offlineAttendanceResponseSchema.parse(await (await cleared).json())
  expect(acknowledgement.outcomes).toMatchObject([{ kind: 'saved', current: { status: null } }])
  await page.reload()
  await expect(attendance(11)).toBeVisible()
  await expect(attendance(12)).toHaveCount(0)
  await expect(last).toHaveCount(0)

  await page.emulateMedia({ reducedMotion: 'reduce' })
  const reduced = attendance(14).getByRole('button', { name: ar ? 'حاضر' : 'present' })
  const neighbor = attendance(15).getByRole('button', { name: ar ? 'حاضر' : 'present' })
  await reduced.focus()
  await page.keyboard.press('Space')
  // After reload, a previous sync may still be reconciling while this mark is saved.
  await page.waitForTimeout(250)
  await expect(reduced).toHaveAttribute('aria-pressed', 'true')
  await expect(attendance(14)).toHaveCount(0)
  await expect(neighbor).toBeFocused()

  await neighbor.click()
  await expect(attendance(15)).toHaveCount(0)
  await expect(attendance(16).getByRole('button', { name: ar ? 'حاضر' : 'present' })).toBeFocused()
})
