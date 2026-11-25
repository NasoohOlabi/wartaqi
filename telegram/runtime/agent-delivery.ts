import type { Auth } from '../auth'
import { Mcp } from '../transport'
import { createAgent, type AgentAccess } from './agent'
import { fallbackReply } from './agent-copy'
import { createAgentModel } from './agent-model'
import type { assistantConfig } from './assistant'

export function createAgentDelivery(
  origin: string,
  auth: Auth,
  config: ReturnType<typeof assistantConfig>,
) {
  const run = createAgent(config ? createAgentModel(config) : undefined)
  const reject = (reason: string) =>
    console.info(JSON.stringify({ message: 'agent_rejected', reason }))
  const deliver = async (
    chat: string,
    question: string,
    lang: 'en' | 'ar',
    say: (chat: string, text: string) => Promise<void>,
  ) => {
    if (/\p{Script=Arabic}{3}/u.test(question)) lang = 'ar'
    else if (/[a-z]{3}/i.test(question)) lang = 'en'
    let stillLinked = () => false
    const start = performance.now()
    const access = async (): Promise<AgentAccess> => {
      const student = auth.student(chat)
      if (student) {
        stillLinked = () => auth.student(chat) === student
        return {
          kind: 'student',
          call: async (name) => {
            if (name !== 'get_my_attendance' || !stillLinked()) throw new Error('Access changed')
            return auth.phoneRequest({ action: 'attendance', token: student })
          },
        }
      }
      const teacher = auth.teacherMcp(chat)
      const token = teacher?.token ?? auth.currentAccessToken(chat)
      if (!token) return { kind: 'guest' }
      stillLinked = () =>
        teacher ? auth.teacherMcp(chat)?.token === token : auth.currentAccessToken(chat) === token
      const mcp = new Mcp(origin, token, teacher?.secret)
      const tools = await mcp.tools()
      const call = async (name: string, args: Record<string, unknown>) => {
        if (!stillLinked()) throw new Error('Access changed')
        return mcp.call(name, args)
      }
      if (tools.has('get_my_attendance')) return { kind: 'student', call }
      return { kind: 'staff', tools, call }
    }
    const result = await run({ question, lang, access })
    const changed = result.source === 'tool' && !stillLinked()
    await say(chat, changed ? fallbackReply(lang, 'denied') : result.text)
    console.info(
      JSON.stringify({
        message: 'agent_delivery',
        language: lang,
        source: changed ? 'fallback' : result.source,
        reason: changed ? 'access_changed' : result.reason,
        tool: result.tool,
        elapsedMs: Math.round(performance.now() - start),
      }),
    )
  }
  return { deliver, reject }
}
