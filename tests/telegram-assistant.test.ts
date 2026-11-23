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

test('private questions reach inference while protected flows stay separate', async () => {
  const requests: string[] = []
  const messages: string[] = []
  globalThis.fetch = Object.assign(
    async (input: string | URL | Request, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body))
      const url = new URL(input instanceof Request ? input.url : input)
      if (url.origin === 'https://api.telegram.org') {
        messages.push(body.text)
        return Response.json({ ok: true, result: { message_id: 1 } })
      }
      expect(url.href).toBe('https://model.example/v1/chat/completions')
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
    worker.store.set('phone-input:1', 1)
    await worker.process(update(1, 'Are you Bonsai and can you call tools?'))
    expect(requests).toHaveLength(1)
    expect(requests[0]).toContain('Are you Bonsai and can you call tools?')
    expect(requests[0]).toContain('powered by model')
    expect(worker.store.get('phone-input:1')).toBe(1)
    expect(messages[0]).toBe('Use /menu.')
    await worker.process(update(4, '12345678'))
    await worker.process(update(5, 'hidden', 1, { forward_date: 1 }))
    await worker.process(update(6, 'x'.repeat(1001)))
    await worker.process(update(6, 'duplicate'))
    await worker.process(update(7, '/unknown question'))
    await worker.process(update(8, '/ask question'))
    await worker.process(update(9, 'My password is hunter2'))
    await worker.process(update(10, 'My code is \u0661\u0662\u0663\u0664\u0665\u0666\u0667\u0668'))
    await worker.process(update(11, '0912'))
    expect(messages.at(-1)).toContain('Check the digits')
    worker.store.set('phone:1', { user: 1, message: 2 })
    await worker.process(update(12, 'While sharing a contact'))
    worker.store.delete('phone:1')
    worker.store.set('link:1', { pending: true })
    await worker.process(update(13, 'While linking'))
    worker.store.delete('link:1')
    worker.store.set('oauth-active:1', 'exchange')
    await worker.process(update(14, 'While exchanging'))
    worker.store.delete('oauth-active:1')
    expect(requests).toHaveLength(1)
    await worker.process(update(15, 'How do I reset my password?'))
    expect(requests).toHaveLength(2)
    worker.store.set('generation:1', 'existing')
    worker.store.set('prompt:1', {
      user: 1,
      generation: 'existing',
      action: { route: 'nb.search' },
    })
    await worker.process(update(16, 'Student name'))
    expect(worker.store.get('prompt:1')).toBeNull()
    await worker.process({
      update_id: 17,
      message: {
        message_id: 17,
        chat: { id: 1, type: 'group' },
        from: { id: 1 },
        text: 'group question',
      },
    })
    await worker.process(update(18, '/help'))
    expect(messages.at(-1)).toContain('for three days')
    await worker.process(update(19, '/help', 2, { from: { id: 2, language_code: 'ar' } }))
    expect(messages.at(-1)).toContain('لمدة ثلاثة أيام')
    expect(requests).toHaveLength(2)
  } finally {
    worker.store.close()
  }
})

test('transcripts follow delivery and exclude chat IDs and obvious secrets', async () => {
  const bodies: Array<Record<string, unknown>> = []
  const order: string[] = []
  let failDelivery = false
  globalThis.fetch = Object.assign(
    async (input: string | URL | Request, init?: RequestInit) => {
      const url = new URL(input instanceof Request ? input.url : input)
      if (url.origin === 'https://model.example') {
        return Response.json({
          choices: [{ finish_reason: 'stop', message: { content: 'Contact 1234567.' } }],
        })
      }
      expect(url.origin).toBe('https://wartaq.example')
      expect(url.pathname).toBe('/api/telegram/conversations')
      expect(init?.redirect).toBe('error')
      expect(init?.signal).toBeDefined()
      expect(new Headers(init?.headers).get('authorization')).toBe(`Bearer ${'s'.repeat(32)}`)
      order.push('transcript')
      bodies.push(JSON.parse(String(init?.body)))
      return new Response(null, { status: failDelivery ? 503 : 204 })
    },
    { preconnect: originalFetch.preconnect },
  )
  const assistant = createAssistant(
    assistantConfig({
      LM_STUDIO_BASE_URL: 'https://model.example',
      LM_STUDIO_MODEL: 'model',
      LM_STUDIO_API_KEY: 'key',
    }),
    { origin: 'https://wartaq.example', secret: 's'.repeat(32) },
  )
  const say = async (_chat: string, reply: string) => {
    order.push('delivery')
    expect(reply).toBe('Contact 1234567.')
    if (failDelivery) throw new Error('private delivery error')
  }
  await assistant.deliver('987654321', 'Call 7654321', 'en', say)
  failDelivery = true
  await assistant.deliver('987654321', 'Another question', 'en', say)
  expect(order).toEqual(['delivery', 'transcript', 'delivery', 'transcript'])
  expect(bodies.map((body) => body.delivery)).toEqual(['sent', 'failed'])
  expect(bodies[0].question).toBe('Call [redacted]')
  expect(bodies[0].reply).toBe('Contact [redacted].')
  expect(bodies[0].conversationId).toMatch(/^[a-f0-9]{64}$/)
  expect(bodies[0].conversationId).toBe(bodies[1].conversationId)
  expect(JSON.stringify(bodies)).not.toContain('987654321')
})
