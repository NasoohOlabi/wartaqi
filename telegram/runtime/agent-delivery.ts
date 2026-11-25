import { createHash, randomBytes } from 'node:crypto'
import { z } from 'zod'
import type { Auth } from '../auth'
import type { Storage } from '../storage'
import { Mcp } from '../transport'
import { fallbackReply } from './agent-copy'
import { agentContextSchema, createProductionAgent, productionCannedReply } from './agent-graph'
import { createProductionModel } from './agent-inference'
import { agentCatalog, validateAgentCall } from './agent-tools'
import type { assistantConfig } from './assistant'
import type { telegramApi } from './telegram'
import { offlineAttendanceResponseSchema } from '../../shared/offlineAttendance'
import { offlineStudentOutcomeSchema } from '../../shared/offlineStudent'

const contextSchema = z.object({ grant: z.string(), context: agentContextSchema })
const pendingSchema = z.object({
  handle: z.string(),
  chat: z.string(),
  user: z.number(),
  message: z.number(),
  grant: z.string(),
  epoch: z.string(),
  tool: z.string(),
  args: z.record(z.string(), z.unknown()),
  lang: z.enum(['en', 'ar']),
})
const receiptSchema = pendingSchema.pick({ handle: true, user: true, message: true })
const nonce = () => randomBytes(18).toString('base64url')

export function createAgentDelivery(
  origin: string,
  auth: Auth,
  config: ReturnType<typeof assistantConfig>,
  store: Storage,
  api: ReturnType<typeof telegramApi>,
) {
  const run = createProductionAgent(config ? createProductionModel(config) : undefined)
  const reject = (reason: string) =>
    console.info(JSON.stringify({ message: 'agent_rejected', reason }))
  const grant = (chat: string) => {
    const student = auth.student(chat)
    const teacher = auth.teacherMcp(chat)
    const token = student ?? teacher?.token ?? auth.currentAccessToken(chat)
    const kind = student ? 'student' : teacher ? 'teacher' : token ? 'oauth' : 'guest'
    return {
      kind,
      token,
      fingerprint: createHash('sha256')
        .update(`${kind}:${token ?? ''}`)
        .digest('hex'),
    }
  }
  const invalidate = (chat: string) => {
    store.delete(`agent-epoch:${chat}`)
    store.delete(`agent-context:${chat}`)
    store.delete(`agent-pending:${chat}`)
  }
  const deliver = async (
    chat: string,
    question: string,
    lang: 'en' | 'ar',
    say: (chat: string, text: string) => Promise<void>,
  ) => {
    if (/\p{Script=Arabic}{3}/u.test(question)) lang = 'ar'
    else if (/[a-z]{3}/i.test(question)) lang = 'en'
    const epoch = nonce()
    store.set(`agent-epoch:${chat}`, epoch, Date.now() + 30 * 60_000)
    store.delete(`agent-pending:${chat}`)
    const initial = grant(chat)
    const valid = () =>
      store.get(`agent-epoch:${chat}`) === epoch && grant(chat).fingerprint === initial.fingerprint
    const old = contextSchema.safeParse(store.get(`agent-context:${chat}`))
    const context =
      old.success && old.data.grant === initial.fingerprint ? old.data.context : undefined
    try {
      const canned = productionCannedReply(question, lang)
      if (canned) {
        if (valid()) await say(chat, canned)
        return
      }
      const teacher = initial.kind === 'teacher' ? auth.teacherMcp(chat) : null
      const mcp =
        initial.token && initial.kind !== 'student'
          ? new Mcp(origin, initial.token, teacher?.secret)
          : null
      const tools =
        initial.kind === 'student'
          ? [
              {
                name: 'get_my_attendance',
                description: 'View your own attendance',
                kind: 'read' as const,
                inputSchema: {
                  type: 'object' as const,
                  properties: {},
                  additionalProperties: false,
                },
              },
            ]
          : mcp
            ? agentCatalog(await mcp.catalog())
            : []
      if (!valid()) return
      const called = new Set<string>()
      const result = await run({
        question,
        lang,
        tools,
        context,
        call: async (name, args) => {
          if (!valid()) throw new Error('Access changed')
          called.add(name)
          const value =
            initial.kind === 'student' && initial.token && name === 'get_my_attendance'
              ? await auth.phoneRequest({ action: 'attendance', token: initial.token })
              : mcp
                ? await mcp.call(name, args)
                : (() => {
                    throw new Error('Access denied')
                  })()
          if (!valid()) throw new Error('Access changed')
          return value
        },
      })
      if (!valid()) {
        if (store.get(`agent-epoch:${chat}`) === epoch)
          await say(chat, fallbackReply(lang, 'denied'))
        return
      }
      if (called.size && mcp) {
        const fresh = new Set(agentCatalog(await mcp.catalog()).map((tool) => tool.name))
        if (!valid() || [...called].some((name) => !fresh.has(name))) {
          if (store.get(`agent-epoch:${chat}`) === epoch)
            await say(chat, fallbackReply(lang, 'denied'))
          return
        }
      }
      store.set(
        `agent-context:${chat}`,
        { grant: initial.fingerprint, context: result.context },
        Date.now() + 30 * 60_000,
      )
      if (result.kind === 'answer') {
        await say(chat, result.text)
        return
      }
      const handle = nonce()
      const sent = z.object({ message_id: z.number() }).parse(
        await api('sendMessage', {
          chat_id: chat,
          text: result.text,
          reply_markup: {
            inline_keyboard: [
              [
                { text: lang === 'ar' ? 'تأكيد' : 'Confirm', callback_data: `agent:c:${handle}` },
                { text: lang === 'ar' ? 'إلغاء' : 'Cancel', callback_data: `agent:x:${handle}` },
              ],
            ],
          },
        }),
      )
      if (!valid()) return
      store.set(
        `agent-pending:${chat}`,
        {
          handle,
          chat,
          user: Number(chat),
          message: sent.message_id,
          grant: initial.fingerprint,
          epoch,
          tool: result.tool,
          args: result.args,
          lang,
        },
        Date.now() + 10 * 60_000,
      )
    } catch {
      if (valid()) await say(chat, fallbackReply(lang, 'unavailable'))
    }
  }
  const callback = async (
    chat: string,
    user: number,
    message: number,
    data: string,
    lang: 'en' | 'ar',
    say: (chat: string, text: string) => Promise<void>,
  ) => {
    const match = /^agent:([cx]):([A-Za-z0-9_-]+)$/.exec(data)
    if (!match) return false
    const parsed = pendingSchema.safeParse(store.get(`agent-pending:${chat}`))
    if (
      !parsed.success ||
      parsed.data.handle !== match[2] ||
      parsed.data.chat !== chat ||
      parsed.data.user !== user ||
      parsed.data.message !== message
    ) {
      const receipt = receiptSchema.safeParse(store.get(`agent-receipt:${chat}`))
      const unknown =
        receipt.success &&
        receipt.data.handle === match[2] &&
        receipt.data.user === user &&
        receipt.data.message === message
      await say(
        chat,
        unknown
          ? lang === 'ar'
            ? '\u0627\u0644\u0646\u062a\u064a\u062c\u0629 \u063a\u064a\u0631 \u0645\u0624\u0643\u062f\u0629. \u062a\u062d\u0642\u0642 \u0645\u0646\u0647\u0627 \u0642\u0628\u0644 \u0627\u0644\u0645\u062d\u0627\u0648\u0644\u0629.'
            : 'The outcome is unknown. Check the result before trying again.'
          : lang === 'ar'
            ? '\u0627\u0646\u062a\u0647\u062a \u0635\u0644\u0627\u062d\u064a\u0629 \u0627\u0644\u062a\u0623\u0643\u064a\u062f. \u0627\u0633\u062a\u062e\u062f\u0645 /menu.'
            : 'This approval has expired. Use /menu.',
      )
      return true
    }
    const pending = parsed.data
    // Synchronous consumption occurs before any asynchronous work, preventing replay.
    store.delete(`agent-pending:${chat}`)
    const valid = () =>
      store.get(`agent-epoch:${chat}`) === pending.epoch &&
      grant(chat).fingerprint === pending.grant
    if (match[1] === 'x') {
      invalidate(chat)
      await say(chat, pending.lang === 'ar' ? 'تم الإلغاء.' : 'Cancelled.')
      return true
    }
    let dispatched = false
    try {
      const current = grant(chat)
      const teacher = current.kind === 'teacher' ? auth.teacherMcp(chat) : null
      if (!current.token || !valid()) throw new Error('Access changed')
      const mcp = new Mcp(origin, current.token, teacher?.secret)
      const tools = agentCatalog(await mcp.catalog())
      const tool = tools.find((item) => item.name === pending.tool && item.kind === 'write')
      if (!tool || !valid()) throw new Error('Access changed')
      const args = validateAgentCall(tool, pending.args)
      if (!valid()) throw new Error('Access changed')
      store.set(
        `agent-receipt:${chat}`,
        { handle: pending.handle, user, message },
        Date.now() + 10 * 60_000,
      )
      dispatched = true
      const result = await mcp.call(tool.name, args)
      if (!valid()) return true
      if (tool.name === 'sync_attendance') {
        const parsedResult = offlineAttendanceResponseSchema.parse(result)
        const records = z
          .array(z.object({ event: z.object({ id: z.string() }) }))
          .parse(args.records)
        if (
          !records.length ||
          records.some(
            (record) => !parsedResult.outcomes.some((outcome) => outcome.id === record.event.id),
          )
        )
          throw new Error('Incomplete attendance outcome')
        if (
          records.some(
            (record) =>
              !parsedResult.outcomes.some(
                (outcome) =>
                  outcome.id === record.event.id && ['saved', 'unchanged'].includes(outcome.kind),
              ),
          )
        ) {
          store.delete(`agent-receipt:${chat}`)
          await say(
            chat,
            pending.lang === 'ar'
              ? 'تغير الحضور أو تعذر حفظه. راجع النتيجة قبل المحاولة.'
              : 'Attendance changed or could not be saved. Review before trying again.',
          )
          return true
        }
      }
      if (tool.name === 'sync_student') {
        const outcome = offlineStudentOutcomeSchema.parse(result)
        const operation = z
          .discriminatedUnion('kind', [
            z.object({ kind: z.literal('note'), event: z.object({ id: z.string() }) }),
            z.object({ kind: z.literal('contact'), id: z.string() }),
          ])
          .parse(args.operation)
        const id = operation.kind === 'note' ? operation.event.id : operation.id
        if (outcome.id !== id) throw new Error('Unexpected student outcome')
        if (outcome.kind !== 'saved') {
          store.delete(`agent-receipt:${chat}`)
          await say(
            chat,
            pending.lang === 'ar'
              ? '\u062a\u063a\u064a\u0631 \u0627\u0644\u0633\u062c\u0644 \u0623\u0648 \u062a\u0639\u0630\u0631 \u062d\u0641\u0638\u0647. \u0631\u0627\u062c\u0639 \u0627\u0644\u0646\u062a\u064a\u062c\u0629.'
              : 'The record changed or could not be saved. Review before trying again.',
          )
          return true
        }
      }
      store.delete(`agent-receipt:${chat}`)
      await say(
        chat,
        pending.lang === 'ar'
          ? '\u062a\u0645 \u0625\u0631\u0633\u0627\u0644 \u0627\u0644\u0637\u0644\u0628. \u0627\u0641\u062a\u062d \u0627\u0644\u0642\u0627\u0626\u0645\u0629 \u0644\u0644\u062a\u062d\u0642\u0642 \u0645\u0646 \u0627\u0644\u0646\u062a\u064a\u062c\u0629.'
          : 'Request submitted. Open the menu to check the result.',
      )
    } catch {
      if (valid())
        await say(
          chat,
          dispatched
            ? pending.lang === 'ar'
              ? 'النتيجة غير مؤكدة. تحقق منها قبل المحاولة مجدداً.'
              : 'The outcome is unknown. Check the result before trying again.'
            : pending.lang === 'ar'
              ? 'لم يعد الطلب متاحاً. افتح القائمة.'
              : 'This request is no longer available. Use /menu.',
        )
    }
    return true
  }
  return { deliver, reject, callback, invalidate }
}
