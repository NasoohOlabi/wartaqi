import { z } from 'zod'
import { setTimeout as sleep } from 'node:timers/promises'
import { listenFetch } from '../server/http'
import { Auth } from './auth'
import { Storage } from './storage'
import { createHttp } from './runtime/http'
import { createNavigation } from './runtime/navigation'
import { telegramApi } from './runtime/telegram'
import { createUpdates } from './runtime/updates'
import { assistantConfig, createAssistant } from './runtime/assistant'
import { createAgentDelivery } from './runtime/agent-delivery'

const configSchema = z.object({
  TELEGRAM_BOT_TOKEN: z.string().min(1),
  WARTAQI_ORIGIN: z.url(),
  TELEGRAM_PUBLIC_ORIGIN: z.url(),
  TELEGRAM_DB_PATH: z.string().min(1),
  TELEGRAM_STORAGE_KEY: z.string().min(1),
  TELEGRAM_WORKER_SECRET: z.string().min(32).optional(),
  TELEGRAM_LANGGRAPH_ENABLED: z.enum(['0', '1']).optional(),
  PORT: z.coerce.number().int().min(1).max(65535).default(3000),
})
export function createWorker(env: Record<string, string | undefined>) {
  const parsed = configSchema.safeParse(env)
  if (!parsed.success) throw new Error('Invalid Telegram worker configuration')
  const config = parsed.data
  for (const value of [config.WARTAQI_ORIGIN, config.TELEGRAM_PUBLIC_ORIGIN]) {
    const url = new URL(value)
    if (url.protocol !== 'https:' || url.origin !== value)
      throw new Error('Origins must be HTTPS origins without a trailing slash')
  }
  const store = new Storage(config.TELEGRAM_DB_PATH, config.TELEGRAM_STORAGE_KEY)
  const auth = new Auth(
    store,
    config.WARTAQI_ORIGIN,
    config.TELEGRAM_PUBLIC_ORIGIN,
    config.TELEGRAM_WORKER_SECRET,
  )
  const api = telegramApi(config.TELEGRAM_BOT_TOKEN)
  const navigation = createNavigation(store, auth, api, config.WARTAQI_ORIGIN)
  const model = assistantConfig(env)
  const agentEnabled = config.TELEGRAM_LANGGRAPH_ENABLED === '1'
  const agent = agentEnabled
    ? createAgentDelivery(config.WARTAQI_ORIGIN, auth, model, store, api)
    : undefined
  const assistant =
    agent ??
    createAssistant(
      model,
      config.TELEGRAM_WORKER_SECRET
        ? { origin: config.WARTAQI_ORIGIN, secret: config.TELEGRAM_WORKER_SECRET }
        : undefined,
    )
  const { process, notifyFailure } = createUpdates(
    store,
    auth,
    api,
    navigation,
    assistant,
    agentEnabled,
    agent,
  )
  let ready = false
  return {
    store,
    process,
    fetch: createHttp(auth, () => ready),
    poll: async () => {
      let lastPrune = 0
      for (;;) {
        try {
          if (Date.now() - lastPrune > 300_000) {
            store.prune()
            lastPrune = Date.now()
          }
          const offset = z.number().nullable().parse(store.get('offset')) ?? 0
          const updates = z.array(z.unknown()).parse(
            await api('getUpdates', {
              offset,
              timeout: ready ? 30 : 1,
              allowed_updates: ['message', 'callback_query'],
            }),
          )
          ready = true
          for (const update of updates) {
            try {
              await process(update, true)
            } catch {
              await notifyFailure(update)
            }
          }
        } catch {
          ready = false
          await sleep(3000)
        }
      }
    },
  }
}

if (import.meta.main) {
  if (process.argv.includes('--assistant-smoke')) {
    const ok = await createAssistant(assistantConfig(process.env)).smoke('smoke')
    process.exit(ok ? 0 : 1)
  }
  const worker = createWorker(process.env)
  const config = assistantConfig(process.env)
  if (config) void createAssistant(config).smoke('startup')
  await listenFetch(worker.fetch, { port: Number(process.env.PORT ?? 3000), hostname: '0.0.0.0' })
  await worker.poll()
}
