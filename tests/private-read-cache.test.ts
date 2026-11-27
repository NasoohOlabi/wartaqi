import { expect, test } from 'bun:test'
import { neonAccountSchema } from '../shared/neon'
import { authorizeLocalGrant, clearOfflineAccess } from '../src/offlineAccess'

test('private read memory is purged on account and grant changes and ignores stale completions', async () => {
  const originalWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  const originalStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage')
  const originalFetch = globalThis.fetch
  const storage = new Map<string, string>()
  const windowTarget = new EventTarget()
  Object.defineProperty(windowTarget, 'location', { value: new URL('http://localhost/') })
  Object.defineProperty(globalThis, 'window', { configurable: true, value: windowTarget })
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
      removeItem: (key: string) => storage.delete(key),
    },
  })
  const seen: (string | null)[] = []
  let pending: ReturnType<typeof Promise.withResolvers<Response>> | undefined
  globalThis.fetch = Object.assign(
    async (_input: string | URL | Request, init?: RequestInit) => {
      const validator = new Headers(init?.headers).get('if-none-match')
      seen.push(validator)
      if (pending && String(_input).endsWith('/api/notebook')) return pending.promise
      return validator
        ? new Response(null, { status: 304 })
        : Response.json({ value: seen.length }, { headers: { ETag: 'W/"one"' } })
    },
    { preconnect: originalFetch.preconnect },
  )
  try {
    const { request } = await import('../src/neon')
    const account = neonAccountSchema.parse({
      role: 'student',
      id: '439a3c8c-50d4-48cf-9b47-9ecd2b6189aa',
      name: 'Synthetic',
      canRead: true,
      canWrite: false,
      expiresAt: null,
    })
    authorizeLocalGrant('one', account)
    expect(await request('/api/student/attendance', 'one')).toEqual({ value: 1 })
    expect(await request('/api/student/attendance', 'one')).toEqual({ value: 1 })
    expect(seen).toEqual([null, 'W/"one"'])
    pending = Promise.withResolvers<Response>()
    const stale = request('/api/notebook', 'one')
    authorizeLocalGrant('two', account)
    pending.resolve(Response.json({ value: 'stale' }, { headers: { ETag: 'W/"old"' } }))
    await stale
    pending = undefined
    expect(await request('/api/student/attendance', 'two')).toEqual({ value: 4 })
    expect(seen[3]).toBeNull()
    authorizeLocalGrant('two', { ...account, id: 'c68e0569-dd55-4071-b12f-ed6cb566fae4' })
    await request('/api/student/attendance', 'two')
    expect(seen[4]).toBeNull()
    pending = Promise.withResolvers<Response>()
    const notebook = request('/api/notebook', 'two')
    await request('/api/me', 'two')
    pending.resolve(Response.json({ value: 'notebook' }, { headers: { ETag: 'W/"notebook"' } }))
    await notebook
    pending = undefined
    expect(await request('/api/notebook', 'two')).toEqual({ value: 'notebook' })
    expect(seen.at(-1)).toBe('W/"notebook"')
    clearOfflineAccess()
  } finally {
    globalThis.fetch = originalFetch
    if (originalWindow) Object.defineProperty(globalThis, 'window', originalWindow)
    else Reflect.deleteProperty(globalThis, 'window')
    if (originalStorage) Object.defineProperty(globalThis, 'localStorage', originalStorage)
    else Reflect.deleteProperty(globalThis, 'localStorage')
  }
})
