import { z } from 'zod'
import { phoneSchema } from '../../shared/invitations'
import { Auth } from '../auth'
import { isBotRoute, type BotRoute } from '../routes'
import { Storage } from '../storage'
import type { createNavigation } from './navigation'
import type { telegramApi } from './telegram'

const updateSchema = z.object({
  update_id: z.number().int(),
  message: z
    .object({
      message_id: z.number(),
      chat: z.object({ id: z.number(), type: z.string() }),
      from: z.object({ id: z.number(), language_code: z.string().optional() }).optional(),
      text: z.string().optional(),
      contact: z.object({ phone_number: z.string(), user_id: z.number().optional() }).optional(),
      reply_to_message: z.object({ message_id: z.number() }).optional(),
      forward_origin: z.unknown().optional(),
      forward_date: z.number().optional(),
    })
    .optional(),
  callback_query: z
    .object({
      id: z.string(),
      from: z.object({ id: z.number() }),
      data: z.string().optional(),
      message: z
        .object({ message_id: z.number(), chat: z.object({ id: z.number(), type: z.string() }) })
        .optional(),
    })
    .optional(),
})
const actionSchema = z.object({
  route: z.custom<BotRoute>(isBotRoute),
  args: z.record(z.string(), z.string()).optional(),
})
const choiceSchema = z.object({
  chat: z.string(),
  user: z.number(),
  message: z.number(),
  generation: z.string(),
  action: actionSchema,
})
export function createUpdates(
  store: Storage,
  auth: Auth,
  api: ReturnType<typeof telegramApi>,
  navigation: ReturnType<typeof createNavigation>,
) {
  const { language, say, show, requestPhone } = navigation
  const process = async (raw: unknown) => {
    const update = updateSchema.parse(raw)
    const previous = z.number().nullable().parse(store.get('offset')) ?? 0
    if (update.update_id < previous) return
    // Mark before handling: a crash or uncertain response must never replay a mutation.
    store.set('offset', update.update_id + 1, Date.now() + 6 * 24 * 60 * 60_000)
    const callback = update.callback_query
    if (callback) {
      await api('answerCallbackQuery', { callback_query_id: callback.id }).catch(() => {})
      const message = callback.message
      if (
        !message ||
        message.chat.type !== 'private' ||
        message.chat.id !== callback.from.id ||
        !callback.data
      )
        return
      const chat = String(message.chat.id)
      const key = `button:${callback.data}`
      const parsedChoice = choiceSchema.safeParse(store.get(key))
      const choice = parsedChoice.success ? parsedChoice.data : null
      if (
        !choice ||
        choice.chat !== chat ||
        choice.user !== callback.from.id ||
        choice.message !== message.message_id ||
        store.get(`generation:${chat}`) !== choice.generation
      ) {
        await say(
          chat,
          language(chat) === 'ar'
            ? 'انتهت صلاحية هذا الخيار. استخدم /menu.'
            : 'This choice has expired. Use /menu.',
        )
        return
      }
      store.delete(key)
      store.delete(`generation:${chat}`)
      store.delete(`prompt:${chat}`)
      await show(chat, callback.from.id, choice.action, undefined, message.message_id)
      return
    }
    const message = update.message
    if (
      !message ||
      message.chat.type !== 'private' ||
      !message.from ||
      message.chat.id !== message.from.id
    )
      return
    const chat = String(message.chat.id)
    const pendingPhone = z
      .object({ user: z.number(), message: z.number(), expected: z.string().optional() })
      .safeParse(store.get(`phone:${chat}`))
    if (message.contact) {
      if (!pendingPhone.success) return
      if (
        pendingPhone.data.user !== message.from.id ||
        message.contact.user_id !== message.from.id ||
        message.forward_origin ||
        message.forward_date ||
        message.reply_to_message?.message_id !== pendingPhone.data.message
      ) {
        await requestPhone(chat, message.from.id, pendingPhone.data.expected)
        return
      }
      const rawPhone = message.contact.phone_number
      const phone = phoneSchema.safeParse(
        /^[1-9][0-9]{6,14}$/.test(rawPhone) ? `+${rawPhone}` : rawPhone,
      )
      if (
        !phone.success ||
        (pendingPhone.data.expected && phone.data !== pendingPhone.data.expected)
      ) {
        store.delete(`phone:${chat}`)
        await api('sendMessage', {
          chat_id: chat,
          text:
            language(chat) === 'ar'
              ? 'لم يتطابق الرقم. استخدم /menu أو /sharephone للمحاولة مجدداً.'
              : 'The number did not match. Use /menu or /sharephone to try again.',
          reply_markup: { remove_keyboard: true },
        })
        return
      }
      store.delete(`phone:${chat}`)
      await api('sendMessage', {
        chat_id: chat,
        text: language(chat) === 'ar' ? 'جارٍ تسجيل الدخول…' : 'Signing in…',
        reply_markup: { remove_keyboard: true },
      }).catch(() => {})
      try {
        await auth.loginPhone(chat, phone.data)
      } catch {
        await say(
          chat,
          language(chat) === 'ar'
            ? 'يتطلب دخول الطالب رقمًا مؤكّدًا في وارتق يطابق رقم تيليجرام. للمعلّمين، استخدم /menu ثم «ربط الحساب».'
            : 'Student phone sign-in requires a verified Wartaq number matching your Telegram number. Teachers can use /menu then Link account.',
        )
        return
      }
      await show(chat, message.from.id, { route: 'nb.mine' })
      return
    }
    if (!message.text) return
    const text = message.text.trim()
    if (store.get(`lang:${chat}`) === null && message.from.language_code?.startsWith('ar'))
      store.set(`lang:${chat}`, 'ar')
    const command = text.split(/\s/)[0].split('@')[0]
    const dismissPhone = async () => {
      store.delete(`phone-input:${chat}`)
      if (!pendingPhone.success) return
      store.delete(`phone:${chat}`)
      await api('sendMessage', {
        chat_id: chat,
        text: language(chat) === 'ar' ? 'تم إغلاق مشاركة الرقم.' : 'Phone sharing closed.',
        reply_markup: { remove_keyboard: true },
      }).catch(() => {})
    }
    const isBrowserCode = /^\d{8}$/.test(text)
    const hadPendingCode = auth.hasPendingCode(chat)
    if (isBrowserCode && hadPendingCode && auth.redeem(chat, text)) {
      await dismissPhone()
      await show(chat, message.from.id, { route: 'home' })
      return
    }
    if (pendingPhone.success && !text.startsWith('/') && !(hadPendingCode && isBrowserCode)) {
      await requestPhone(chat, message.from.id, pendingPhone.data.expected)
      return
    }
    if (
      !text.startsWith('/') &&
      store.get(`phone-input:${chat}`) === message.from.id &&
      !hadPendingCode &&
      !auth.isLinked(chat)
    ) {
      const phone = phoneSchema.safeParse(text)
      if (!phone.success) {
        await say(
          chat,
          language(chat) === 'ar'
            ? 'أدخل رقم هاتف صحيحاً مع رمز البلد، مثل +963912345678، أو استخدم /sharephone.'
            : 'Enter a valid phone number with country code, such as +963912345678, or use /sharephone.',
        )
        return
      }
      await requestPhone(chat, message.from.id, phone.data)
      return
    }
    if (isBrowserCode && (hadPendingCode || !store.get(`credentials:${chat}`))) {
      await say(
        chat,
        language(chat) === 'ar'
          ? 'الرمز غير صحيح أو انتهت صلاحيته. حاول مجدداً، أو استخدم /cancel ثم /start لبدء الربط من جديد.'
          : 'That code is incorrect or expired. Try again, or use /cancel then /start to link again.',
      )
      return
    }
    if (command === '/sharephone') {
      if (auth.phoneAvailable() && !auth.isLinked(chat)) await requestPhone(chat, message.from.id)
      else await show(chat, message.from.id, { route: 'home' })
      return
    }
    if (command === '/unlink') {
      await show(chat, message.from.id, { route: 'sys.unlink' })
      return
    }
    if (command === '/language') {
      await dismissPhone()
      await show(chat, message.from.id, { route: 'sys.language' })
      return
    }
    if (command === '/cancel') {
      store.delete(`phone:${chat}`)
      store.delete(`phone-input:${chat}`)
      auth.cancel(chat)
      store.delete(`prompt:${chat}`)
      store.delete(`generation:${chat}`)
      await api('sendMessage', {
        chat_id: chat,
        text:
          language(chat) === 'ar'
            ? 'تم الإلغاء. استخدم /menu لفتح القائمة.'
            : 'Cancelled. Use /menu to open the menu.',
        ...(pendingPhone.success ? { reply_markup: { remove_keyboard: true } } : {}),
      })
      return
    }
    if (command === '/help') {
      await say(
        chat,
        language(chat) === 'ar'
          ? '/menu القائمة، /sharephone مشاركة رقم الهاتف، /cancel إلغاء، /language اللغة، /unlink إلغاء الربط'
          : '/menu opens the menu, /sharephone shares your phone, /cancel cancels, /language changes language, /unlink disconnects.',
      )
      return
    }
    if (command === '/start' || command === '/menu') {
      await dismissPhone()
      store.delete(`prompt:${chat}`)
      await show(chat, message.from.id, { route: 'home' })
      return
    }
    const parsedPrompt = z
      .object({ user: z.number(), generation: z.string(), action: actionSchema })
      .safeParse(store.get(`prompt:${chat}`))
    const prompt = parsedPrompt.success ? parsedPrompt.data : null
    if (
      !text.startsWith('/') &&
      prompt?.user === message.from.id &&
      store.get(`generation:${chat}`) === prompt.generation
    ) {
      store.delete(`generation:${chat}`)
      store.delete(`prompt:${chat}`)
      await show(chat, message.from.id, prompt.action, text)
      return
    }
    await say(
      chat,
      language(chat) === 'ar' ? 'استخدم /menu لفتح القائمة.' : 'Use /menu to open the menu.',
    )
  }
  const notifyFailure = async (update: unknown) => {
    const parsed = updateSchema.safeParse(update)
    if (parsed.success) {
      const message = parsed.data.message ?? parsed.data.callback_query?.message
      const user = parsed.data.message?.from ?? parsed.data.callback_query?.from
      if (message?.chat.type === 'private' && user?.id === message.chat.id) {
        const chat = String(message.chat.id)
        await say(
          chat,
          language(chat) === 'ar'
            ? 'تعذر إكمال الطلب. تحقق من النتيجة قبل المحاولة مجدداً، أو استخدم /menu.'
            : 'The request could not be completed. Check the result before trying again, or use /menu.',
        ).catch(() => {})
      }
    }
  }
  return { process, notifyFailure }
}
