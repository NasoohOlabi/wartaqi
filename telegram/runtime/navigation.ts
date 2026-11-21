import { randomBytes } from 'node:crypto'
import { z } from 'zod'
import { neonStudentAttendanceSchema } from '../../shared/neon'
import { Auth } from '../auth'
import { renderBot } from '../menu'
import { Storage } from '../storage'
import { Mcp } from '../transport'
import type { Action, Screen } from '../types'
import type { telegramApi } from './telegram'

const secret = () => randomBytes(18).toString('base64url')

export function createNavigation(
  store: Storage,
  auth: Auth,
  api: ReturnType<typeof telegramApi>,
  origin: string,
) {
  const language = (chat: string): 'en' | 'ar' => (store.get(`lang:${chat}`) === 'ar' ? 'ar' : 'en')
  const say = async (chat: string, text: string) => {
    await api('sendMessage', { chat_id: chat, text })
  }
  const show = async (
    chat: string,
    user: number,
    action: Action,
    input?: string,
    messageId?: number,
  ) => {
    let lang = language(chat)
    if (action.route === 'sys.language') {
      lang = lang === 'en' ? 'ar' : 'en'
      store.set(`lang:${chat}`, lang)
      action = { route: 'home' }
    }
    if (action.route === 'sys.phone') {
      if (!auth.phoneAvailable() || auth.isLinked(chat)) return
      store.delete(`prompt:${chat}`)
      store.delete(`generation:${chat}`)
      const sent = z.object({ message_id: z.number() }).parse(
        await api('sendMessage', {
          chat_id: chat,
          text:
            lang === 'ar'
              ? 'شارك رقم تيليجرام المسجّل لحسابك في وارتق. للإلغاء، استخدم /cancel.'
              : 'Share the Telegram number registered for your Wartaq account. Use /cancel to stop.',
          reply_markup: {
            keyboard: [
              [
                {
                  text: lang === 'ar' ? 'مشاركة رقم هاتفي' : 'Share my phone',
                  request_contact: true,
                },
              ],
            ],
            resize_keyboard: true,
            one_time_keyboard: true,
          },
        }),
      )
      store.set(`phone:${chat}`, { user, message: sent.message_id }, Date.now() + 5 * 60_000)
      return
    }
    if (action.route === 'sys.unlink') {
      await auth.unlink(chat)
      store.delete(`generation:${chat}`)
      store.delete(`prompt:${chat}`)
      store.delete(`phone:${chat}`)
      await api('sendMessage', {
        chat_id: chat,
        text: lang === 'ar' ? 'تم إلغاء الربط.' : 'Account unlinked.',
        reply_markup: { remove_keyboard: true },
      })
      return
    }
    if (action.route === 'sys.home') action = { route: 'home' }
    const studentToken = auth.student(chat)
    const credentials = studentToken ? null : await auth.credentials(chat)
    if (!credentials && !studentToken) {
      store.delete(`prompt:${chat}`)
      store.delete(`generation:${chat}`)
      const showPending = async () => {
        await say(
          chat,
          lang === 'ar'
            ? 'أرسل الرمز المكوّن من ٨ أرقام المعروض في المتصفح هنا. للبدء من جديد، استخدم /cancel ثم /start.'
            : 'Send the eight-digit code shown in your browser here. To start again, use /cancel then /start.',
        )
      }
      if (auth.hasPendingCode(chat)) {
        await showPending()
        return
      }
      if (auth.isExchanging(chat)) {
        await say(
          chat,
          lang === 'ar'
            ? 'جارٍ إكمال الربط. انتظر رمز المتصفح، أو استخدم /cancel ثم /start للمحاولة من جديد.'
            : 'Linking is in progress. Wait for the browser code, or use /cancel then /start to try again.',
        )
        return
      }
      let url: string
      try {
        url = await auth.begin(chat)
      } catch (error) {
        if (auth.hasPendingCode(chat)) {
          await showPending()
          return
        }
        throw error
      }
      const phoneButton = auth.phoneAvailable()
        ? {
            text: lang === 'ar' ? 'تسجيل الدخول بالهاتف' : 'Sign in with phone',
            callback_data: secret(),
          }
        : null
      const sent = z.object({ message_id: z.number() }).parse(
        await api('sendMessage', {
          chat_id: chat,
          text: phoneButton
            ? lang === 'ar'
              ? 'اختر طريقة تسجيل الدخول إلى وارتق.'
              : 'Choose how to sign in to Wartaq.'
            : lang === 'ar'
              ? 'مرحباً بك في وارتق. افتح الرابط وسجّل الدخول، ثم أرسل الرمز المعروض هنا.'
              : 'Welcome to Wartaq. Open the link and sign in, then send the displayed code here.',
          reply_markup: {
            inline_keyboard: [
              [{ text: lang === 'ar' ? 'ربط الحساب' : 'Link account', url }],
              ...(phoneButton ? [[phoneButton]] : []),
            ],
          },
        }),
      )
      if (!phoneButton) return
      const generation = secret()
      store.set(`generation:${chat}`, generation)
      store.set(
        `button:${phoneButton.callback_data}`,
        { chat, user, message: sent.message_id, generation, action: { route: 'sys.phone' } },
        Date.now() + 15 * 60_000,
      )
      return
    }
    const mcp = credentials ? new Mcp(origin, credentials.access_token) : null
    let studentData: z.infer<typeof neonStudentAttendanceSchema> | undefined
    let tools: ReadonlySet<string>
    try {
      if (studentToken) {
        studentData = neonStudentAttendanceSchema.parse(
          await auth.phoneRequest({ action: 'attendance', token: studentToken }),
        )
        tools = new Set(['get_my_attendance'])
      } else if (mcp) tools = await mcp.tools()
      else throw new Error('Missing credentials')
    } catch (error) {
      if (
        error instanceof Error &&
        (error.message === 'MCP request failed (401)' ||
          error.message === 'Student request failed (403)')
      )
        await auth.unlink(chat)
      throw error
    }
    const screen: Screen = await renderBot({
      lang,
      route: studentToken && !['home', 'nb.mine'].includes(action.route) ? 'home' : action.route,
      args: action.args ?? {},
      input,
      tools,
      call: async (name, args) => {
        try {
          if (studentData) {
            if (name === 'get_my_attendance') return studentData
            if (name === 'get_account')
              return {
                account: {
                  role: 'student',
                  id: studentData.student.studentId,
                  name: studentData.student.name,
                  canRead: true,
                  canWrite: false,
                  expiresAt: null,
                },
              }
            throw new Error('Unavailable')
          }
          if (!mcp) throw new Error('Missing credentials')
          return await mcp.call(name, args)
        } catch (error) {
          if (error instanceof Error && error.message === 'MCP request failed (401)')
            await auth.unlink(chat)
          throw error
        }
      },
      website: origin,
    })
    if (studentToken && action.route === 'home')
      screen.rows = screen.rows.filter(
        (row) => !row.some((button) => 'action' in button && button.action.route === 'ac.profile'),
      )
    const generation = secret()
    store.set(`generation:${chat}`, generation)
    const entries: { id: string; action: Action }[] = []
    const keyboard = screen.rows.map((row) =>
      row.map((button) => {
        if ('url' in button) return { text: button.label, url: button.url }
        const id = secret()
        entries.push({ id, action: button.action })
        return { text: button.label, callback_data: id }
      }),
    )
    const sent = z.object({ message_id: z.number() }).parse(
      await api(messageId ? 'editMessageText' : 'sendMessage', {
        chat_id: chat,
        ...(messageId ? { message_id: messageId } : {}),
        text: screen.text,
        reply_markup: { inline_keyboard: keyboard },
      }),
    )
    for (const entry of entries)
      store.set(
        `button:${entry.id}`,
        { chat, user, message: sent.message_id, generation, action: entry.action },
        Date.now() + 15 * 60_000,
      )
    if (screen.prompt)
      store.set(
        `prompt:${chat}`,
        { user, generation, action: screen.prompt },
        Date.now() + 15 * 60_000,
      )
    else store.delete(`prompt:${chat}`)
    if (screen.document) await api.document(chat, screen.document)
  }
  return { language, say, show }
}
