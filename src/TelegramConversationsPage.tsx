import { useEffect, useEffectEvent, useRef, useState } from 'react'
import type { z } from 'zod'
import {
  telegramConversationsPath,
  telegramConversationsSchema,
  telegramConversationTurnsPath,
  telegramConversationTurnsSchema,
} from '../shared/telegram-conversations'
import { request } from './neon'
import type { Profile } from './ProfileMenu'
import { SettingsNavigation } from './SettingsNavigation'
import { useLanguage } from './useLanguage'
import { WorkspaceNavigation } from './WorkspaceNavigation'
import { WorkspacePageHeader } from './WorkspacePageHeader'

type Conversations = z.infer<typeof telegramConversationsSchema>
type Turns = z.infer<typeof telegramConversationTurnsSchema>
type Conversation = Conversations['items'][number]

export function TelegramConversationsPage({
  userId,
  profile,
  signOut,
}: {
  userId: string
  profile: Profile
  signOut: () => void
}) {
  const { language } = useLanguage()
  const ar = language === 'ar'
  const [conversations, setConversations] = useState<Conversations | null>(null)
  const [selected, setSelected] = useState<Conversation | null>(null)
  const [turns, setTurns] = useState<Turns | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(false)
  const latest = useRef(0)
  const detailHeading = useRef<HTMLHeadingElement>(null)

  async function loadList(before?: string) {
    const id = ++latest.current
    setLoading(true)
    setError(false)
    try {
      const params = new URLSearchParams()
      if (before) params.set('before', before)
      const data = telegramConversationsSchema.parse(
        await request(`${telegramConversationsPath}?${params}`, userId),
      )
      if (id === latest.current)
        setConversations((previous) => ({
          items: before && previous ? [...previous.items, ...data.items] : data.items,
          nextCursor: data.nextCursor,
        }))
    } catch {
      if (id === latest.current) setError(true)
    } finally {
      if (id === latest.current) setLoading(false)
    }
  }

  async function loadTurns(conversation: Conversation, before?: string) {
    const id = ++latest.current
    setLoading(true)
    setError(false)
    try {
      const params = new URLSearchParams({ conversation: conversation.conversationId })
      if (before) params.set('before', before)
      const data = telegramConversationTurnsSchema.parse(
        await request(`${telegramConversationTurnsPath}?${params}`, userId),
      )
      if (id === latest.current)
        setTurns((previous) => ({
          items: before && previous ? [...previous.items, ...data.items] : data.items,
          nextCursor: data.nextCursor,
        }))
    } catch {
      if (id === latest.current) setError(true)
    } finally {
      if (id === latest.current) setLoading(false)
    }
  }

  const loadInitial = useEffectEvent(() => void loadList())
  useEffect(() => {
    let active = true
    const reads = latest
    queueMicrotask(() => {
      if (active) loadInitial()
    })
    return () => {
      active = false
      reads.current++
    }
  }, [])
  useEffect(() => {
    if (selected) detailHeading.current?.focus()
  }, [selected])
  const date = (value: string) => new Date(value).toLocaleString(ar ? 'ar' : 'en')
  const outcomeLabel = (value: string) => {
    if (!ar) return value.replaceAll('_', ' ')
    const labels: Record<string, string> = {
      ok: 'نجح',
      unconfigured: 'غير مهيأ',
      malformed: 'استجابة غير صالحة',
      tool_request: 'طلب أداة',
      output_limit: 'تجاوز حد الرد',
      incomplete: 'رد غير مكتمل',
      empty: 'رد فارغ',
      link_output: 'الرد يحتوي رابطاً',
      timeout: 'انتهت المهلة',
      network: 'خطأ في الشبكة',
    }
    return labels[value] ?? 'خطأ في الخدمة'
  }

  return (
    <div className="workspace">
      <WorkspaceNavigation administrator profile={profile} leave={signOut} />
      <main id="main-content" className="workspace-main" tabIndex={-1}>
        <WorkspacePageHeader title={ar ? 'محادثات تيليغرام' : 'Telegram conversations'}>
          <SettingsNavigation current="telegram" />
        </WorkspacePageHeader>
        <section className="panel space-y-4" aria-busy={loading}>
          {selected ? (
            <>
              <button
                className="secondary"
                type="button"
                onClick={() => {
                  latest.current++
                  setSelected(null)
                  setTurns(null)
                  setLoading(false)
                  setError(false)
                }}
              >
                {ar ? 'العودة إلى القائمة' : 'Back to list'}
              </button>
              <h2 className="title" ref={detailHeading} tabIndex={-1}>
                {ar ? 'المحادثة' : 'Conversation'}
              </h2>
              {turns &&
                [...turns.items].reverse().map((turn) => (
                  <article
                    key={turn.trace}
                    className="space-y-3 border-b border-slate-200 pb-4 dark:border-slate-700"
                  >
                    <div className="max-w-[85%] rounded-xl bg-slate-100 p-3 dark:bg-slate-800">
                      <strong>{ar ? 'المستخدم' : 'User'}</strong>
                      <p className="whitespace-pre-wrap break-words" dir="auto">
                        {turn.question}
                      </p>
                    </div>
                    <div className="ms-auto max-w-[85%] rounded-xl bg-blue-50 p-3 dark:bg-slate-700">
                      <strong>{ar ? 'المساعد' : 'Assistant'}</strong>
                      <p className="whitespace-pre-wrap break-words" dir="auto">
                        {turn.reply || (ar ? 'لا يوجد رد' : 'No reply')}
                      </p>
                    </div>
                    <p className="muted break-words">
                      {date(turn.receivedAt)} · {ar ? 'التسليم' : 'Delivery'}:{' '}
                      {turn.delivery === 'sent' ? (ar ? 'تم' : 'Sent') : ar ? 'فشل' : 'Failed'} ·{' '}
                      {ar ? 'النتيجة' : 'Outcome'}: {outcomeLabel(turn.outcome)} ·{' '}
                      {ar ? 'المدة' : 'Elapsed'}: {turn.elapsedMs} ms
                    </p>
                    <p className="muted break-words">
                      {ar ? 'النموذج' : 'Model'}: {turn.model || '—'} ·{' '}
                      {ar ? 'إصدار التعليمات' : 'Prompt version'}: {turn.promptVersion || '—'}
                    </p>
                  </article>
                ))}
              {turns && !turns.items.length && !loading && (
                <p>{ar ? 'لا توجد رسائل.' : 'No messages.'}</p>
              )}
              {turns?.nextCursor && (
                <button
                  type="button"
                  className="secondary"
                  disabled={loading}
                  onClick={() => void loadTurns(selected, turns.nextCursor || undefined)}
                >
                  {ar ? 'تحميل الرسائل الأقدم' : 'Load older messages'}
                </button>
              )}
            </>
          ) : (
            <>
              {conversations?.items.map((item) => (
                <div
                  key={item.conversationId}
                  className="flex flex-wrap items-center justify-between gap-3 border-b border-slate-200 py-3 dark:border-slate-700"
                >
                  <div>
                    <strong>{date(item.lastSeenAt)}</strong>
                    <p className="muted">
                      {item.turnCount}{' '}
                      {ar ? 'تبادل' : item.turnCount === 1 ? 'exchange' : 'exchanges'}
                    </p>
                  </div>
                  <button
                    type="button"
                    className="secondary"
                    onClick={() => {
                      setSelected(item)
                      setTurns(null)
                      void loadTurns(item)
                    }}
                  >
                    {ar ? 'فتح المحادثة' : 'Open conversation'}
                  </button>
                </div>
              ))}
              {conversations && !conversations.items.length && !loading && (
                <p>{ar ? 'لا توجد محادثات.' : 'No conversations.'}</p>
              )}
              {conversations?.nextCursor && (
                <button
                  type="button"
                  className="secondary"
                  disabled={loading}
                  onClick={() => void loadList(conversations.nextCursor || undefined)}
                >
                  {ar ? 'تحميل المزيد' : 'Load more'}
                </button>
              )}
            </>
          )}
          {loading && <p role="status">{ar ? 'جارٍ التحميل…' : 'Loading…'}</p>}
          {error && (
            <p role="alert">
              {ar
                ? 'تعذر تحميل المحادثات. حاول مرة أخرى.'
                : 'Could not load conversations. Try again.'}{' '}
              <button
                type="button"
                className="secondary"
                onClick={() => (selected ? void loadTurns(selected) : void loadList())}
              >
                {ar ? 'إعادة المحاولة' : 'Retry'}
              </button>
            </p>
          )}
        </section>
      </main>
    </div>
  )
}
