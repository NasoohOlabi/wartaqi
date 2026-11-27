import { expect, test } from '@playwright/test'
import { clickAccountAction } from './profileActions'
import { mkdir, readFile } from 'node:fs/promises'

test.use({ trace: 'off' })

test('exports active course contacts and offers direct WhatsApp links', async ({
  page,
}, testInfo) => {
  const id = '10000000-0000-4000-8000-000000000001'
  const course = '30000000-0000-4000-8000-000000000001'
  const other = '30000000-0000-4000-8000-000000000002'
  const arabicName = 'أحمد محمد عبد الرحمن عبد الكريم عبد الوهاب Test'
  const teacher = {
    role: 'administrator',
    id,
    name: 'Synthetic Admin',
    canRead: true,
    canWrite: true,
    expiresAt: null,
  }
  const students = [
    { name: 'Alpha, Test', whatsapp: '٠٩١٢٣٤٥٦٧٨', active: true },
    { name: arabicName, whatsapp: '00442012345678', active: true },
    { name: 'Missing Test', whatsapp: '1234567', active: true },
    { name: 'Withdrawn Test', whatsapp: '+12025550123', active: true },
    { name: 'Inactive Test', whatsapp: '+12025550124', active: false },
    { name: 'Other Test', whatsapp: '+12025550125', active: true },
  ].map((student, i) => ({
    ...student,
    kind: 'student',
    studentId: `20000000-0000-4000-8000-00000000000${i + 1}`,
    group: '',
    firstName: student.name.split(' ')[0],
    lastName: student.name.split(' ').slice(1).join(' '),
  }))
  const enrollments = students.map((student, i) => ({
    id: `40000000-0000-4000-8000-00000000000${i + 1}`,
    courseId: i === 5 ? other : course,
    studentId: student.studentId,
    level: '',
    active: i !== 3,
    practical: null,
    theory: null,
  }))
  await page.route('**/api/auth/**', (route) =>
    route.fulfill({
      json: {
        user: { id, name: teacher.name, email: 'synthetic@example.test' },
        session: {
          id: 'synthetic',
          userId: id,
          expiresAt: new Date(Date.now() + 3600000).toISOString(),
        },
      },
    }),
  )
  await page.route('**/api/**', (route) => {
    const path = new URL(route.request().url()).pathname
    if (path.startsWith('/api/auth/')) return route.fallback()
    if (path === '/api/me') return route.fulfill({ json: { account: teacher } })
    return route.fulfill({
      json: {
        teacher,
        students,
        courses: [
          { id: course, name: 'Synthetic Course', open: true },
          { id: other, name: 'Other Course', open: true },
        ],
        enrollments,
        events: [],
      },
    })
  })
  await page.goto('/?page=app&tab=roster')
  await expect(page.locator('.roster-students-table')).toBeVisible()
  await expect(page.getByRole('img', { name: 'Inactive', exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Export contacts' })).toHaveCount(0)
  await page.goto(`/?page=app&tab=roster&course=${course}`)
  const exportButton = page.getByRole('button', { name: 'Export contacts' })
  await expect(exportButton).toBeVisible()
  await expect(
    page.getByRole('status').filter({ hasText: '2 ready to export, 1 without a usable number' }),
  ).toBeVisible()
  const chat = page.getByRole('link', { name: 'WhatsApp chat: ٠٩١٢٣٤٥٦٧٨' })
  await expect(chat).toHaveAttribute('href', 'https://wa.me/963912345678')
  await expect(chat).toHaveText('')
  await expect(chat.locator('svg[aria-hidden="true"]')).toBeVisible()
  await expect(page.getByRole('link', { name: '+442012345678', exact: true })).toHaveAttribute(
    'href',
    'tel:+442012345678',
  )
  await page
    .context()
    .route('https://wa.me/**', (route) =>
      route.fulfill({ status: 200, body: 'Synthetic intercepted destination' }),
    )
  const popupPromise = page.waitForEvent('popup')
  await chat.click()
  const popup = await popupPromise
  await expect(popup).toHaveURL('https://wa.me/963912345678')
  await popup.close()
  await page.getByLabel('Find a student').fill('Alpha')
  await expect(page.getByRole('link', { name: '+442012345678', exact: true })).toHaveCount(0)
  const downloadPromise = page.waitForEvent('download')
  await exportButton.click()
  const download = await downloadPromise
  expect(download.suggestedFilename()).toBe('Synthetic Course-contacts.vcf')
  const content = await readFile(await download.path(), 'utf8')
  const unfolded = content.replaceAll('\r\n ', '')
  expect(unfolded).toContain('FN:Alpha\\, Test\r\n')
  expect(content).toContain('TEL;TYPE=CELL:+963912345678\r\n')
  expect(unfolded).toContain(`FN:${arabicName}\r\n`)
  expect(unfolded).toContain('N:محمد عبد الرحمن عبد الكريم عبد الوهاب Test;أحمد;;;\r\n')
  expect(content).toContain('\r\n ')
  expect(content.split('\r\n').every((line) => Buffer.byteLength(line, 'utf8') <= 75)).toBe(true)
  expect(content).toContain('TEL;TYPE=CELL:+442012345678\r\n')
  expect(content).not.toMatch(/Missing|Withdrawn|Inactive|Other/)
  await page.getByLabel('Find a student').fill('')
  const table = page.locator('.roster-students-table')
  const row = table.locator('tbody tr').filter({ hasText: arabicName })
  for (const ar of [false, true]) {
    if (ar) await clickAccountAction(page, 'العربية')
    const heading = page.getByRole('heading', { name: ar ? 'طلابك' : 'Your students', exact: true })
    const exportLabel = ar ? 'تصدير جهات الاتصال' : 'Export contacts'
    const exportAction = page.getByRole('button', { name: exportLabel, exact: true })
    await expect(exportAction).toHaveText('')
    await expect(exportAction).toHaveAttribute('title', exportLabel)
    await expect(exportAction.locator('svg[aria-hidden="true"]')).toBeVisible()
    const whatsapp = row.getByRole('link', {
      name: ar ? 'محادثة واتساب: +442012345678' : 'WhatsApp chat: +442012345678',
    })
    const status = row.getByRole('img', { name: ar ? 'نشط' : 'Active', exact: true })
    await expect(whatsapp).toHaveAttribute('href', 'https://wa.me/442012345678')
    await expect(whatsapp).toHaveText('')
    for (const width of [1624, 1280, 320, 360, 390, 430]) {
      await page.setViewportSize({ width, height: 844 })
      const exportBounds = await exportAction.boundingBox()
      if (!exportBounds) throw new Error('Export contacts is hidden')
      expect(exportBounds.width).toBeGreaterThanOrEqual(44)
      expect(exportBounds.height).toBeGreaterThanOrEqual(44)
      for (const label of ar
        ? ['خيارات الفرز', 'خيارات التصفية']
        : ['Sort options', 'Filter options']) {
        const control = page.getByRole('button', { name: label, exact: true })
        await expect(control).toBeVisible()
        expect((await control.boundingBox())?.y).toBe(exportBounds.y)
      }
      const headingLines = await heading.evaluate(
        (el) => el.clientHeight / Number.parseFloat(getComputedStyle(el).lineHeight),
      )
      expect(headingLines).toBe(1)
      if (ar || width >= 390) {
        const title = await heading.boundingBox()
        if (!title) throw new Error('Students heading is hidden')
        expect(title.y).toBeGreaterThanOrEqual(exportBounds.y)
        expect(title.y + title.height).toBeLessThanOrEqual(exportBounds.y + exportBounds.height)
      }
      const target = await whatsapp.boundingBox()
      if (!target) throw new Error('WhatsApp link is hidden')
      expect(target.width).toBeGreaterThanOrEqual(44)
      expect(target.height).toBeGreaterThanOrEqual(44)
      const dialLink = row.locator('a[href^="tel:"]')
      const dial = await dialLink.boundingBox()
      if (!dial) throw new Error('Dial link is hidden')
      expect(Math.abs(dial.y + dial.height / 2 - target.y - target.height / 2)).toBeLessThan(1)
      if (ar) expect(target.x + target.width).toBeLessThanOrEqual(dial.x + 1)
      else expect(dial.x + dial.width).toBeLessThanOrEqual(target.x + 1)
      await expect(status).toBeVisible()
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
      ).toBe(true)
      const desktopStatus = row.locator('.roster-student-status')
      if (width > 430) {
        await expect(desktopStatus.getByRole('img')).toBeVisible()
        expect(
          await dialLink.evaluate((link) => {
            const range = document.createRange()
            range.selectNodeContents(link)
            return range.getClientRects().length
          }),
        ).toBe(1)
        continue
      }
      await expect(desktopStatus).toBeHidden()
      const badge = await status.boundingBox()
      const name = await row.locator('.roster-mobile-open bdi').boundingBox()
      if (!badge || !name) throw new Error('Roster name or status is hidden')
      expect(Math.abs(name.y + name.height / 2 - badge.y - badge.height / 2)).toBeLessThan(1)
      if (ar) expect(name.x + name.width).toBeLessThanOrEqual(badge.x + 1)
      else expect(badge.x + badge.width).toBeLessThanOrEqual(name.x + 1)
      expect(
        await table.evaluate((el) => el.scrollWidth <= el.parentElement!.clientWidth + 1),
      ).toBe(true)
      if (width === 390 && testInfo.project.name === 'mobile') {
        await mkdir('.artifacts/verification', { recursive: true })
        await row.evaluate((record) => record.scrollIntoView({ block: 'center' }))
        await row.screenshot({
          path: `.artifacts/verification/course-roster-contact-${ar ? 'ar' : 'en'}.png`,
          scale: 'css',
        })
      }
    }
  }
  const arabicChat = page.getByRole('link', { name: 'محادثة واتساب: ٠٩١٢٣٤٥٦٧٨' })
  await expect(arabicChat).toHaveAttribute('href', 'https://wa.me/963912345678')
  await expect(arabicChat).toHaveText('')
})
