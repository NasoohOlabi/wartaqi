import { expect, test } from 'bun:test'
import { z } from 'zod'
import { runLabCase } from '../scripts/agent-lab'
import { labCases, syntheticAccess } from '../scripts/agent-lab-fixtures'
import { createWorker } from '../telegram/runtime'
import { createAgentModel } from '../telegram/runtime/agent-model'

for (const fixture of labCases) {
  test(`headless Wartaq agent: ${fixture.id}`, async () => {
    const result = await runLabCase(fixture)
    expect(result.source).toBe(fixture.expected)
    if (fixture.includes) expect(result.text).toContain(fixture.includes)
    expect(result.calls).toEqual(fixture.calls ?? [])
    expect(result.text.length).toBeGreaterThan(0)
    expect(result.text.length).toBeLessThan(4096)
    if (fixture.expected === 'canned') expect(result.modelCalls).toBe(0)
  })
}

test('LM Studio tool protocol uses the permitted functions and no record data', async () => {
  const request = Object.assign(
    async (_input: string | URL | Request, init?: RequestInit) => {
      const body: unknown = JSON.parse(String(init?.body))
      expect(body).toMatchObject({
        model: 'test-model',
        tools: [{ function: { name: 'find_students' } }],
        messages: [{ role: 'system' }, { role: 'user', content: 'Find student Demo Lina' }],
      })
      expect(JSON.stringify(body)).not.toContain('get_my_attendance')
      expect(init?.signal).toBeDefined()
      return Response.json({
        choices: [
          {
            message: {
              tool_calls: [
                {
                  type: 'function',
                  function: {
                    name: 'find_students',
                    arguments: '{"query":"Demo Lina"}',
                  },
                },
              ],
            },
          },
        ],
      })
    },
    { preconnect: fetch.preconnect },
  )
  const model = createAgentModel(
    { url: 'https://model.example/v1/chat/completions', model: 'test-model', key: 'synthetic' },
    request,
  )
  expect(await model('Find student Demo Lina', 'en', ['find_students'])).toEqual({
    tool: 'find_students',
    args: { query: 'Demo Lina' },
  })
})

test('private updates use a fresh phone grant and suppress a result if the grant was removed', async () => {
  const original = globalThis.fetch
  const messages: string[] = []
  const calls: string[] = []
  const worker = createWorker({
    TELEGRAM_BOT_TOKEN: 'test',
    WARTAQI_ORIGIN: 'https://wartaq.example',
    TELEGRAM_PUBLIC_ORIGIN: 'https://bot.example',
    TELEGRAM_DB_PATH: ':memory:',
    TELEGRAM_STORAGE_KEY: Buffer.alloc(32).toString('base64'),
    TELEGRAM_WORKER_SECRET: 's'.repeat(32),
    TELEGRAM_LANGGRAPH_ENABLED: '1',
  })
  const synthetic = syntheticAccess('student', 'normal', calls)
  if (synthetic.kind !== 'student') throw new Error('Expected a student fixture')
  let withdraw = false
  globalThis.fetch = Object.assign(
    async (input: string | URL | Request, init?: RequestInit) => {
      const url = new URL(input instanceof Request ? input.url : input)
      const body: unknown = JSON.parse(String(init?.body))
      if (url.origin === 'https://api.telegram.org' && url.pathname.endsWith('/sendMessage')) {
        messages.push(z.object({ text: z.string() }).parse(body).text)
        return Response.json({ ok: true, result: { message_id: 1 } })
      }
      if (url.href === 'https://wartaq.example/api/telegram/student') {
        expect(body).toEqual({ action: 'attendance', token: 'synthetic-grant' })
        if (withdraw) worker.store.delete('student:1')
        return Response.json(await synthetic.call('get_my_attendance', {}))
      }
      throw new Error(`Unexpected test request to ${url.origin}`)
    },
    { preconnect: original.preconnect },
  )
  const send = (id: number, text: string) =>
    worker.process({
      update_id: id,
      message: { message_id: id, chat: { id: 1, type: 'private' }, from: { id: 1 }, text },
    })
  try {
    worker.store.set('student:1', 'synthetic-grant')
    await send(1, 'How do I sign in?')
    expect(calls).toEqual([])
    expect(messages.at(-1)).toContain('/sharephone')
    await send(2, 'Show my attendance')
    expect(messages.at(-1)).toContain('2026-11-23')
    withdraw = true
    await send(3, 'Show my attendance')
    expect(messages.at(-1)).not.toContain('Demo Recitation')
    expect(messages.at(-1)).toContain('/menu')
    expect(calls).toEqual(['get_my_attendance', 'get_my_attendance'])
  } finally {
    globalThis.fetch = original
    worker.store.close()
  }
})
