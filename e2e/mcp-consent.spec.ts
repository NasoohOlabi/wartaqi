import { expect, test } from '@playwright/test'
import { mcpConsentPage } from '../server/mcp-consent'

for (const width of [320, 1440]) {
  for (const language of ['en', 'ar']) {
    for (const path of ['/mcp/login', '/mcp/consent']) {
      test(`MCP setup handoff at ${path} ${width}px ${language}`, async ({ browser, baseURL }) => {
        const context = await browser.newContext({
          viewport: { width, height: 800 },
          storageState: { cookies: [], origins: [] },
        })
        let pending = true
        let consentQuery = ''
        let signInBody: unknown
        await context.route('**/*', async (route) => {
          const request = route.request()
          const url = new URL(request.url())
          if (url.pathname === '/mcp/consent' || url.pathname === '/mcp/login') {
            const page = mcpConsentPage(new Request(url))
            await route.fulfill({
              status: 200,
              headers: Object.fromEntries(page.headers),
              body: await page.text(),
            })
          } else if (url.pathname === '/api/auth/get-session') {
            await route.fulfill({ json: { user: { id: 'synthetic-user', name: 'Synthetic' } } })
          } else if (url.pathname === '/api/auth/sign-in/phone') {
            signInBody = request.postDataJSON()
            await route.fulfill({ status: 401, json: { error: 'Invalid credentials' } })
          } else if (url.pathname === '/api/device-accounts') {
            await route.fulfill({ json: { accounts: [] } })
          } else if (url.pathname === '/api/me') {
            expect(request.headers()['x-user-id']).toBe('synthetic-user')
            await route.fulfill({
              json: { account: { invitationProfile: { requiresSetup: pending } } },
            })
          } else if (url.pathname === '/' && url.search === '?page=app') {
            await route.fulfill({
              contentType: 'text/html',
              body: '<button>Complete setup</button><script>document.querySelector("button").onclick=()=>fetch("/synthetic-complete",{method:"POST"})</script>',
            })
          } else if (url.pathname === '/synthetic-complete') {
            pending = false
            await route.fulfill({ json: { saved: true } })
          } else if (
            url.pathname === '/api/auth/oauth2/consent' ||
            url.pathname === '/api/auth/oauth2/continue'
          ) {
            consentQuery = JSON.parse(request.postData() || '{}').oauth_query
            await route.fulfill({ json: { url: '/synthetic-finished' } })
          } else {
            await route.fulfill({ contentType: 'text/html', body: 'Finished' })
          }
        })
        const page = await context.newPage()
        const query = 'client_id=synthetic&scope=mcp%3Aread&signed=synthetic-signature'
        await page.goto(`${baseURL}${path}?${query}`)
        await expect(page.locator('html')).toHaveAttribute('lang', 'ar')
        await expect(page.locator('html')).toHaveAttribute('dir', 'rtl')
        if (language === 'en') await page.getByRole('button', { name: 'English' }).click()
        const signIn = page.locator('#login summary')
        await signIn.click()
        const form = page.getByRole('form', {
          name: language === 'ar' ? 'تسجيل الدخول إلى حساب آخر' : 'Sign in to another account',
        })
        await expect(form).toHaveAttribute('method', 'post')
        await expect(form.locator('input[name="username"]')).toHaveAttribute(
          'autocomplete',
          'username',
        )
        await expect(form.locator('input[name="password"]')).toHaveAttribute(
          'autocomplete',
          'current-password',
        )
        if (path === '/mcp/login') {
          await form.locator('input[name="username"]').fill('0912345678')
          await form.locator('input[name="password"]').fill('synthetic-password')
          await form.getByRole('button').click()
          await expect(page.getByRole('alert')).toHaveText(
            language === 'ar'
              ? 'تعذر إكمال الطلب. تحقق من البيانات وحاول مرة أخرى.'
              : 'Could not complete the request. Check your details and try again.',
          )
          expect(signInBody).toEqual({
            phone: '0912345678',
            password: 'synthetic-password',
            oauth_query: query,
          })
        }
        await signIn.click()
        const link = page.locator('#setup a')
        await expect(link).toBeVisible()
        expect(await link.getAttribute('target')).toBe('_blank')
        expect(await link.getAttribute('rel')).toContain('noreferrer')
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        )
        const [popup] = await Promise.all([context.waitForEvent('page'), link.click()])
        await popup.waitForLoadState()
        expect(popup.url()).toBe(`${baseURL}/?page=app`)
        await popup.getByText('Complete setup').click()
        await expect.poll(() => pending).toBe(false)
        expect(new URL(page.url()).search.slice(1)).toBe(query)
        await page.reload()
        await expect(link).toBeHidden()
        await expect(page.locator('html')).toHaveAttribute('lang', language)
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        )
        await page.locator(path === '/mcp/consent' ? '#accept' : '#resume').click()
        await expect.poll(() => consentQuery).toBe(query)
        await context.close()
      })
    }
  }
}
