import { z } from 'zod'
import { phoneSchema } from '../../shared/invitations'
import { Auth, StudentRequestError } from '../auth'
import { isBotRoute, type BotRoute } from '../routes'
import { Storage } from '../storage'
import type { createNavigation } from './navigation'
import type { telegramApi } from './telegram'
import type { createAssistant } from './assistant'

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
  assistant: ReturnType<typeof createAssistant>,
) {
  const { language, say, show, requestPhone } = navigation
  const assistantChats = new Set<string>()
  const process = async (raw: unknown, detach = false) => {
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
    const phoneIssue = (en: string, ar: string) =>
      language(chat) === 'ar'
        ? `${ar}\n\nيمكنك أيضاً استخدام /linkaccount. سجّل الدخول ووافق في المتصفح، ثم أرسل الرمز من ثمانية أرقام هنا.`
        : `${en}\n\nYou can also use /linkaccount. Sign in and approve in your browser, then paste the eight-digit code here.`
    const pendingPhone = z
      .object({ user: z.number(), message: z.number(), expected: z.string().optional() })
      .safeParse(store.get(`phone:${chat}`))
    if (message.contact) {
      if (!pendingPhone.success) {
        if (!auth.isLinked(chat))
          await say(
            chat,
            phoneIssue(
              'The phone sharing request expired. Use /sharephone to try again.',
              'انتهت صلاحية طلب مشاركة الرقم. استخدم /sharephone للمحاولة مجدداً.',
            ),
          )
        return
      }
      if (
        pendingPhone.data.user !== message.from.id ||
        message.contact.user_id !== message.from.id ||
        message.forward_origin ||
        message.forward_date ||
        message.reply_to_message?.message_id !== pendingPhone.data.message
      ) {
        await say(
          chat,
          phoneIssue(
            'Share your own contact using the button below so Telegram can confirm it.',
            'شارك رقمك من الزر أدناه حتى يتمكن تيليجرام من تأكيده.',
          ),
        )
        await requestPhone(chat, message.from.id, pendingPhone.data.expected)
        return
      }
      const rawPhone = message.contact.phone_number.trim()
      const phone = phoneSchema.safeParse(
        /^[1-9][0-9 ()-]{6,39}$/.test(rawPhone) ? `+${rawPhone}` : rawPhone,
      )
      if (
        !phone.success ||
        (pendingPhone.data.expected && phone.data !== pendingPhone.data.expected)
      ) {
        store.delete(`phone:${chat}`)
        await api('sendMessage', {
          chat_id: chat,
          text: !phone.success
            ? phoneIssue(
                'Couldn’t read the number Telegram shared. Try /sharephone again.',
                'تعذّرت قراءة الرقم الذي شاركه تيليجرام. جرّب /sharephone مجدداً.',
              )
            : phoneIssue(
                'The shared number differs from the one you typed. Check the digits and use /menu to try again.',
                'الرقم المُشارك يختلف عمّا كتبته. تحقق من الأرقام واستخدم /menu للمحاولة مجدداً.',
              ),
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
      } catch (error) {
        const denied = error instanceof StudentRequestError && error.status === 403
        await say(
          chat,
          denied
            ? phoneIssue(
                'Students, check that your Telegram number matches the phone number on your roster record. Teachers, use your verified Wartaq sign-in number.',
                'للطلاب، تأكّدوا من أن رقم تيليجرام يطابق رقم الهاتف المسجل في قائمة الطلاب. للمعلمين، استخدموا رقم تسجيل الدخول المؤكد في وارتق.',
              )
            : phoneIssue(
                'Phone sign-in is temporarily unavailable. Try /sharephone again.',
                'تعذر تسجيل الدخول بالهاتف مؤقتاً. حاول مجدداً باستخدام /sharephone.',
              ),
        )
        return
      }
      await show(chat, message.from.id, { route: auth.teacher(chat) ? 'home' : 'nb.mine' })
      return
    }
    if (!message.text) return
    const text = message.text.trim()
    if (store.get(`lang:${chat}`) === null && message.from.language_code?.startsWith('ar'))
      store.set(`lang:${chat}`, 'ar')
    const commandToken = text.split(/\s/, 1)[0]
    const command = commandToken.split('@')[0]
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
      await say(
        chat,
        phoneIssue('Use the button to share your number.', 'استخدم الزر لمشاركة رقمك.'),
      )
      await requestPhone(chat, message.from.id, pendingPhone.data.expected)
      return
    }
    if (
      /^(?=.*\p{Decimal_Number})[\p{Decimal_Number} ()+-]+$/u.test(text) &&
      store.get(`phone-input:${chat}`) === message.from.id &&
      !hadPendingCode &&
      !auth.isLinked(chat)
    ) {
      const phone = phoneSchema.safeParse(text)
      if (!phone.success) {
        await say(
          chat,
          phoneIssue(
            'Check the digits and try again. Syrian mobile numbers can start with 09 or +9639.',
            'تحقق من الأرقام وحاول مجدداً. يمكن أن تبدأ الأرقام السورية بـ 09 أو +9639.',
          ),
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
          ? 'الرمز غير صحيح أو انتهت صلاحيته. حاول مجدداً، أو استخدم /cancel ثم /linkaccount لبدء الربط من جديد.'
          : 'That code is incorrect or expired. Try again, or use /cancel then /linkaccount to link again.',
      )
      return
    }
    if (command === '/linkaccount') {
      await dismissPhone()
      await show(chat, message.from.id, { route: 'sys.linkaccount' })
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
          ? '/menu القائمة، /linkaccount ربط الحساب عبر المتصفح، /sharephone مشاركة رقم الهاتف، /cancel إلغاء، /language اللغة، /unlink إلغاء الربط. اكتب سؤالك مباشرة للمساعدة. يمكن لمسؤولي وارتق مراجعة أسئلتك للمساعد وردوده لمدة ثلاثة أيام. تجنّب إرسال المعلومات الشخصية في أسئلتك.'
          : '/menu opens the menu, /linkaccount links through your browser, /sharephone shares your phone, /cancel cancels, /language changes language, /unlink disconnects. Type a question directly for help. Administrators may review assistant questions and replies for three days; leave out personal information.',
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
    if (!text.startsWith('/')) {
      const question = text
      const reason =
        message.forward_origin || message.forward_date
          ? 'forwarded'
          : pendingPhone.success || auth.hasPendingCode(chat) || auth.isExchanging(chat)
            ? 'pending_auth'
            : !question
              ? 'empty'
              : question.length > 1000
                ? 'input_limit'
                : /(?:\p{Decimal_Number}[\s()+-]*){8,}|(?:password|passcode|token|secret|كلمة السر|رمز التحقق)\s*(?:[:=]|\bis\b)\s*\S+/iu.test(
                      question,
                    )
                  ? 'sensitive_input'
                  : assistantChats.has(chat) || assistantChats.size >= 3
                    ? 'busy'
                    : ''
      if (reason) {
        assistant.reject(reason)
        const ar = language(chat) === 'ar'
        if (reason === 'busy') {
          await say(
            chat,
            ar ? 'المساعد مشغول الآن. حاول بعد قليل.' : 'The assistant is busy. Try again shortly.',
          )
        } else if (reason === 'pending_auth') {
          await say(
            chat,
            ar
              ? 'أكمل تسجيل الدخول أو استخدم /cancel، ثم أرسل سؤالك.'
              : 'Finish signing in or use /cancel, then send your question.',
          )
        } else {
          await say(
            chat,
            ar
              ? 'أرسل سؤالك مباشرة دون معلومات شخصية.'
              : 'Send a short question. Leave out personal information.',
          )
        }
        return
      }
      assistantChats.add(chat)
      const task = assistant
        .deliver(chat, question, language(chat), say)
        .catch(() => {
          assistant.reject('delivery_failure')
        })
        .finally(() => {
          assistantChats.delete(chat)
        })
      if (!detach) await task
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
