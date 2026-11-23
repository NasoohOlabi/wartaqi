import { afterEach, expect, test } from 'bun:test'
import { createWorker } from '../telegram/runtime'
import { assistantConfig, createAssistant } from '../telegram/runtime/assistant'

const originalFetch = globalThis.fetch
afterEach(() => {
  globalThis.fetch = originalFetch
})

test('provider and delivery failures never log raw content or errors', async () => {
  const logs: string[] = []
  const originalInfo = console.info
  console.info = (...args: unknown[]) => {
    logs.push(args.map(String).join(' '))
  }
  globalThis.fetch = Object.assign(
    async () => {
      throw new Error('private-provider-error')
    },
    { preconnect: originalFetch.preconnect },
  )
  const assistant = createAssistant(
    assistantConfig({
      LM_STUDIO_BASE_URL: 'https://model.example',
      LM_STUDIO_MODEL: 'model',
      LM_STUDIO_API_KEY: 'private-key',
    }),
  )
  try {
    await assistant.deliver('private-chat', 'private-question', 'en', async () => {
      throw new Error('private-delivery-error')
    })
    const output = logs.join('\n')
    expect(output).toContain('network')
    expect(output).toContain('failed')
    expect(output).not.toContain('private-')
  } finally {
    console.info = originalInfo
  }
})

test('provider body failures and unsafe finish metadata produce safe diagnostics', async () => {
  const logs: string[] = []
  const originalInfo = console.info
  console.info = (event: string) => logs.push(event)
  const assistant = createAssistant(
    assistantConfig({
      LM_STUDIO_BASE_URL: 'https://model.example/v1/',
      LM_STUDIO_MODEL: 'model',
      LM_STUDIO_API_KEY: 'private-key',
    }),
  )
  const responses = [
    () => new Response('private-malformed-body'),
    () =>
      new Response(
        new ReadableStream({
          start(controller) {
            controller.error(new DOMException('private-timeout-error', 'TimeoutError'))
          },
        }),
      ),
    () =>
      Response.json({
        choices: [{ finish_reason: 'private-finish', message: { content: 'private-answer' } }],
      }),
  ]
  try {
    for (const response of responses) {
      globalThis.fetch = Object.assign(async () => response(), {
        preconnect: originalFetch.preconnect,
      })
      expect(await assistant.smoke('smoke')).toBe(false)
    }
    const output = logs.join('\n')
    expect(output).toContain('"outcome":"malformed"')
    expect(output).toContain('"outcome":"timeout"')
    expect(output).toContain('"finishReason":"other"')
    expect(output).not.toContain('private-')
  } finally {
    console.info = originalInfo
  }
})

test('only explicit private questions reach inference and reports are chat bound', async () => {
  const requests: string[] = []
  const messages: string[] = []
  globalThis.fetch = Object.assign(
    async (input: string | URL | Request, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body))
      if (String(input).includes('api.telegram.org')) {
        messages.push(body.text)
        return Response.json({ ok: true, result: { message_id: 1 } })
      }
      requests.push(JSON.stringify(body))
      return Response.json({
        choices: [{ finish_reason: 'stop', message: { content: 'Use /menu.' } }],
      })
    },
    { preconnect: originalFetch.preconnect },
  )
  const worker = createWorker({
    TELEGRAM_BOT_TOKEN: 'token',
    WARTAQI_ORIGIN: 'https://wartaq.example',
    TELEGRAM_PUBLIC_ORIGIN: 'https://bot.example',
    TELEGRAM_DB_PATH: ':memory:',
    TELEGRAM_STORAGE_KEY: Buffer.alloc(32).toString('base64'),
    LM_STUDIO_BASE_URL: 'https://model.example/v1',
    LM_STUDIO_MODEL: 'model',
    LM_STUDIO_API_KEY: 'secret',
  })
  const update = (id: number, text: string, chat = 1, extra = {}) => ({
    update_id: id,
    message: {
      message_id: id,
      chat: { id: chat, type: 'private' },
      from: { id: chat },
      text,
      ...extra,
    },
  })
  try {
    worker.store.set('prompt:1', { user: 1, generation: 'existing', action: { route: 'home' } })
    worker.store.set('phone-input:1', 1)
    await worker.process(update(1, '/ask How do I open the menu?'))
    expect(requests).toHaveLength(1)
    expect(requests[0]).toContain('How do I open the menu?')
    expect(worker.store.get('prompt:1')).not.toBeNull()
    expect(worker.store.get('phone-input:1')).toBe(1)
    const reference = messages[0].match(/\/report ([\w-]+)/)?.[1]
    expect(reference).toBeDefined()
    await worker.process(update(2, `/report ${reference}`, 2))
    expect(messages.at(-1)).toContain('invalid')
    await worker.process(update(3, `/report ${reference}`))
    expect(messages.at(-1)).toContain('recorded')
    await worker.process(update(4, '/ask 12345678'))
    await worker.process(update(5, '/ask hidden', 1, { forward_date: 1 }))
    await worker.process(update(6, '/ask ' + 'x'.repeat(1001)))
    await worker.process(update(6, '/ask duplicate'))
    await worker.process({
      update_id: 7,
      message: {
        message_id: 7,
        chat: { id: 1, type: 'group' },
        from: { id: 1 },
        text: '/ask group',
      },
    })
    await worker.process(update(8, '/ask My code is ١٢٣٤٥٦٧٨'))
    worker.store.set('phone:1', { user: 1, message: 2 })
    await worker.process(update(9, '/ask While sharing a contact'))
    expect(requests).toHaveLength(1)
  } finally {
    worker.store.close()
  }
})
