import { expect, test } from '@playwright/test'
import { clickAccountAction } from './profileActions'
import { readFile } from 'node:fs/promises'

test.use({ trace: 'off' })

test('exports active course contacts and offers direct WhatsApp links', async ({ page }) => {
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
  for (const [title, exportLabel, sortLabel, filterLabel] of [
    ['Your students', 'Export contacts', 'Sort options', 'Filter options'],
    ['طلابك', 'تصدير جهات الاتصال', 'خيارات الفرز', 'خيارات التصفية'],
  ]) {
    if (title === 'طلابك') await clickAccountAction(page, 'العربية')
    const heading = page.getByRole('heading', { name: title, exact: true })
    const exportAction = page.getByRole('button', { name: exportLabel, exact: true })
    await expect(exportAction).toHaveText('')
    await expect(exportAction).toHaveAttribute('title', exportLabel)
    await expect(exportAction.locator('svg[aria-hidden="true"]')).toBeVisible()
    for (const width of [1624, 320, 360, 390, 430]) {
      await page.setViewportSize({ width, height: 844 })
      const target = await exportAction.boundingBox()
      expect(target?.width).toBeGreaterThanOrEqual(44)
      expect(target?.height).toBeGreaterThanOrEqual(44)
      for (const label of [sortLabel, filterLabel]) {
        const control = page.getByRole('button', { name: label, exact: true })
        await expect(control).toBeVisible()
        expect((await control.boundingBox())?.y).toBe(target?.y)
      }
      const headingLines = await heading.evaluate(
        (el) => el.clientHeight / Number.parseFloat(getComputedStyle(el).lineHeight),
      )
      expect(headingLines).toBe(1)
      if (title === 'طلابك' || width >= 390) {
        const headingBox = await heading.boundingBox()
        expect(headingBox?.y).toBeGreaterThanOrEqual(target?.y ?? 0)
        expect((headingBox?.y ?? 0) + (headingBox?.height ?? 0)).toBeLessThanOrEqual(
          (target?.y ?? 0) + (target?.height ?? 0),
        )
      }
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
      ).toBe(true)
      if (width <= 430)
        expect(
          await page
            .locator('.roster-students-table')
            .evaluate((el) => el.scrollWidth <= el.parentElement!.clientWidth + 1),
        ).toBe(true)
    }
  }
  const arabicChat = page.getByRole('link', { name: 'محادثة واتساب: ٠٩١٢٣٤٥٦٧٨' })
  await expect(arabicChat).toHaveAttribute('href', 'https://wa.me/963912345678')
  await expect(arabicChat).toHaveText('')
  for (const width of [1624, 320, 390, 430]) {
    await page.setViewportSize({ width, height: 844 })
    const target = await arabicChat.boundingBox()
    expect(target?.width).toBeGreaterThanOrEqual(44)
    expect(target?.height).toBeGreaterThanOrEqual(44)
  }
})
