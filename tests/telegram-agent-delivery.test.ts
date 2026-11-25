import { expect, test } from 'bun:test'
import { z } from 'zod'
import { createWorker } from '../telegram/runtime'

const env = {
  TELEGRAM_BOT_TOKEN: 'test',
  WARTAQI_ORIGIN: 'https://wartaq.example',
  TELEGRAM_PUBLIC_ORIGIN: 'https://bot.example',
  TELEGRAM_DB_PATH: ':memory:',
  TELEGRAM_STORAGE_KEY: Buffer.alloc(32).toString('base64'),
  TELEGRAM_WORKER_SECRET: 's'.repeat(32),
  TELEGRAM_LANGGRAPH_ENABLED: '1',
  LM_STUDIO_BASE_URL: 'https://model.example/v1',
  LM_STUDIO_MODEL: 'test',
  LM_STUDIO_API_KEY: 'synthetic',
}

const tool = {
  name: 'update_profile',
  description: 'Update profile',
  inputSchema: {
    type: 'object',
    properties: { name: { type: 'string' } },
    required: ['name'],
    additionalProperties: false,
  },
}

test('worker approves an exact write once and binds it to the originating message', async () => {
  const original = globalThis.fetch
  const sent: { text: string; message_id: number; data?: string }[] = []
  const calls: unknown[] = []
  let failWrite = false
  let holdModel: Promise<void> | undefined
  let modelStarted: (() => void) | undefined
  globalThis.fetch = Object.assign(
    async (input: string | URL | Request, init?: RequestInit) => {
      const url = new URL(input instanceof Request ? input.url : input)
      const body = z.record(z.string(), z.unknown()).parse(JSON.parse(String(init?.body)))
      if (url.origin === 'https://api.telegram.org') {
        if (url.pathname.endsWith('/sendMessage')) {
          const markup = z
            .object({ inline_keyboard: z.array(z.array(z.object({ callback_data: z.string() }))) })
            .optional()
            .parse(body.reply_markup)
          sent.push({
            text: z.string().parse(body.text),
            message_id: sent.length + 1,
            data: markup?.inline_keyboard[0]?.[0]?.callback_data,
          })
          return Response.json({ ok: true, result: { message_id: sent.length } })
        }
        return Response.json({ ok: true, result: true })
      }
      if (url.origin === 'https://model.example') {
        modelStarted?.()
        await holdModel
        return Response.json({
          choices: [
            {
              message: {
                role: 'assistant',
                content: null,
                tool_calls: [
                  {
                    id: 'call',
                    type: 'function',
                    function: {
                      name: 'update_profile',
                      arguments: JSON.stringify({ name: 'Synthetic' }),
                    },
                  },
                ],
              },
            },
          ],
        })
      }
      if (url.origin === 'https://wartaq.example') {
        if (body.method === 'tools/list') return Response.json({ result: { tools: [tool] } })
        if (body.method === 'tools/call') {
          calls.push(body.params)
          if (failWrite) throw new Error('Lost response')
          return Response.json({ result: { structuredContent: { ok: true } } })
        }
      }
      throw new Error('Unexpected request')
    },
    { preconnect: original.preconnect },
  )
  const worker = createWorker(env)
  worker.store.set('teacher:1', 'synthetic-grant')
  const message = (id: number, text: string) =>
    worker.process({
      update_id: id,
      message: { message_id: id, chat: { id: 1, type: 'private' }, from: { id: 1 }, text },
    })
  const click = (id: number, data: string, messageId: number) =>
    worker.process({
      update_id: id,
      callback_query: {
        id: String(id),
        from: { id: 1 },
        data,
        message: { message_id: messageId, chat: { id: 1, type: 'private' } },
      },
    })
  try {
    await message(1, 'Change my name to Synthetic')
    const approval = sent.at(-1)
    expect(approval?.text).toContain('Confirm')
    const data = z.string().parse(approval?.data)
    await click(2, data, 999)
    expect(calls).toHaveLength(0)
    await click(3, data, approval?.message_id ?? 0)
    expect(calls).toEqual([{ name: 'update_profile', arguments: { name: 'Synthetic' } }])
    await click(4, data, approval?.message_id ?? 0)
    expect(calls).toHaveLength(1)
    await message(5, 'Change my name to Synthetic')
    const revoked = sent.at(-1)
    worker.store.delete('teacher:1')
    await click(6, z.string().parse(revoked?.data), revoked?.message_id ?? 0)
    expect(calls).toHaveLength(1)
    worker.store.set('teacher:1', 'synthetic-grant')
    await message(7, 'Change my name to Synthetic')
    const cancelled = sent.at(-1)
    const cancelData = z.string().parse(cancelled?.data)
    await click(8, cancelData.replace('agent:c:', 'agent:x:'), cancelled?.message_id ?? 0)
    await click(9, cancelData, cancelled?.message_id ?? 0)
    expect(calls).toHaveLength(1)
    await message(10, 'Change my name to Synthetic')
    const uncertain = sent.at(-1)
    const uncertainData = z.string().parse(uncertain?.data)
    failWrite = true
    await click(11, uncertainData, uncertain?.message_id ?? 0)
    expect(sent.at(-1)?.text).toContain('outcome is unknown')
    await click(12, uncertainData, uncertain?.message_id ?? 0)
    expect(sent.at(-1)?.text).toContain('outcome is unknown')
    expect(calls).toHaveLength(2)
    let releaseModel = () => {}
    holdModel = new Promise<void>((resolve) => {
      releaseModel = resolve
    })
    const started = new Promise<void>((resolve) => {
      modelStarted = resolve
    })
    const inFlight = message(13, 'Change my name to Synthetic')
    await started
    await message(14, '/cancel')
    const count = sent.length
    releaseModel()
    await inFlight
    expect(sent).toHaveLength(count)
    expect(calls).toHaveLength(2)
  } finally {
    globalThis.fetch = original
    worker.store.close()
  }
})
import { createHash } from 'node:crypto'
import { Auth } from '../telegram/auth'
import { Storage } from '../telegram/storage'
import { createAgentDelivery } from '../telegram/runtime/agent-delivery'
import { telegramApi } from '../telegram/runtime/telegram'

test('attendance conflict is never reported as saved', async () => {
  const original = globalThis.fetch
  const store = new Storage(':memory:', env.TELEGRAM_STORAGE_KEY)
  const auth = new Auth(
    store,
    env.WARTAQI_ORIGIN,
    env.TELEGRAM_PUBLIC_ORIGIN,
    env.TELEGRAM_WORKER_SECRET,
  )
  const id = '18f7539e-04ca-49d2-89f4-2ffbb28f9790'
  const replies: string[] = []
  store.set('teacher:1', 'synthetic-grant')
  store.set('agent-epoch:1', 'epoch')
  store.set('agent-pending:1', {
    handle: 'handle',
    chat: '1',
    user: 1,
    message: 7,
    grant: createHash('sha256').update('teacher:synthetic-grant').digest('hex'),
    epoch: 'epoch',
    tool: 'sync_attendance',
    lang: 'en',
    args: { records: [{ event: { id } }] },
  })
  globalThis.fetch = Object.assign(
    async (_input: string | URL | Request, init?: RequestInit) => {
      const body = z.object({ method: z.string() }).parse(JSON.parse(String(init?.body)))
      if (body.method === 'tools/list')
        return Response.json({
          result: {
            tools: [
              {
                name: 'sync_attendance',
                description: '',
                inputSchema: {
                  type: 'object',
                  properties: { records: { type: 'array' } },
                  required: ['records'],
                },
              },
            ],
          },
        })
      return Response.json({
        result: { structuredContent: { outcomes: [{ id, kind: 'conflict', current: null }] } },
      })
    },
    { preconnect: original.preconnect },
  )
  try {
    const delivery = createAgentDelivery(env.WARTAQI_ORIGIN, auth, null, store, telegramApi('test'))
    await delivery.callback('1', 1, 7, 'agent:c:handle', 'en', async (_chat, text) => {
      replies.push(text)
    })
    expect(replies).toEqual([
      'Attendance changed or could not be saved. Review before trying again.',
    ])
  } finally {
    globalThis.fetch = original
    store.close()
  }
})

test('worker sends a model-selected read through the teacher MCP and returns the answer', async () => {
  const original = globalThis.fetch
  let modelCalls = 0
  let toolCalls = 0
  const sent: string[] = []
  globalThis.fetch = Object.assign(
    async (input: string | URL | Request, init?: RequestInit) => {
      const url = new URL(input instanceof Request ? input.url : input)
      const body = z.record(z.string(), z.unknown()).parse(JSON.parse(String(init?.body)))
      if (url.origin === 'https://api.telegram.org') {
        sent.push(z.string().parse(body.text))
        return Response.json({ ok: true, result: { message_id: 1 } })
      }
      if (url.origin === 'https://model.example') {
        modelCalls++
        return Response.json({
          choices: [
            {
              message:
                modelCalls === 1
                  ? {
                      role: 'assistant',
                      content: null,
                      tool_calls: [
                        {
                          id: 'read',
                          type: 'function',
                          function: { name: 'list_courses', arguments: '{}' },
                        },
                      ],
                    }
                  : { role: 'assistant', content: 'Demo course is open.' },
            },
          ],
        })
      }
      if (url.pathname === '/api/telegram/teacher-mcp') {
        if (body.method === 'tools/list')
          return Response.json({
            result: {
              tools: [
                {
                  name: 'list_courses',
                  description: '',
                  inputSchema: { type: 'object', additionalProperties: false },
                },
              ],
            },
          })
        toolCalls++
        return Response.json({
          result: { structuredContent: [{ name: 'Demo course', open: true }] },
        })
      }
      throw new Error('Unexpected request')
    },
    { preconnect: original.preconnect },
  )
  const worker = createWorker(env)
  worker.store.set('teacher:1', 'synthetic-grant')
  try {
    await worker.process({
      update_id: 1,
      message: {
        message_id: 1,
        chat: { id: 1, type: 'private' },
        from: { id: 1 },
        text: 'Which courses are open?',
      },
    })
    expect(modelCalls).toBe(2)
    expect(toolCalls).toBe(1)
    expect(sent.at(-1)).toContain('Demo course')
  } finally {
    worker.store.close()
    globalThis.fetch = original
  }
})
