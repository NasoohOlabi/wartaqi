import { useEffect, useRef, useState } from 'react'
import { flushSync } from 'react-dom'
import { invitationResponseSchema } from '../shared/invitations'
import { CopyableInvitation } from './InviteAccounts'
import { request } from './neon'
import { useLanguage } from './useLanguage'
import { setQuery } from './queryState'
import type { Profile } from './ProfileMenu'
import { WorkspaceNavigation } from './WorkspaceNavigation'
import { WorkspacePageHeader } from './WorkspacePageHeader'

type ViewerState =
  | { kind: 'ready' | 'loading' | 'unavailable' }
  | { kind: 'done'; invitations: ReturnType<typeof invitationResponseSchema.parse>['invitations'] }

export function InvitationMessageViewer({
  userId,
  requestId,
  profile,
  signOut,
}: {
  userId: string
  requestId: string
  profile: Profile
  signOut: () => void
}) {
  const { language } = useLanguage()
  const ar = language === 'ar'
  const [state, setState] = useState<ViewerState>(
    requestId ? { kind: 'ready' } : { kind: 'unavailable' },
  )
  const generation = useRef(0)
  const sending = useRef(false)

  useEffect(() => {
    const invalidate = () => {
      generation.current++
    }
    const clear = () => {
      invalidate()
      flushSync(() => {
        setState((previous) => (previous.kind === 'ready' ? previous : { kind: 'unavailable' }))
      })
    }
    window.addEventListener('pagehide', clear)
    return () => {
      invalidate()
      window.removeEventListener('pagehide', clear)
    }
  }, [])

  async function reveal() {
    if (sending.current) return
    sending.current = true
    const current = generation.current
    setState({ kind: 'loading' })
    try {
      const result = invitationResponseSchema.parse(
        await request('/api/admin/accounts/invite-message', userId, { requestId }),
      )
      if (generation.current !== current) return
      setState({ kind: 'done', invitations: result.invitations })
    } catch {
      if (generation.current === current) setState({ kind: 'unavailable' })
    } finally {
      sending.current = false
    }
  }

  const back = {
    label: ar
      ? '\u0627\u0644\u0639\u0648\u062f\u0629 \u0625\u0644\u0649 \u0627\u0644\u062d\u0633\u0627\u0628\u0627\u062a'
      : 'Back to accounts',
    onClick: () =>
      setQuery({ view: 'settings', settingsSection: '', account: '', inviteMessage: '' }, true),
  }

  return (
    <div className="workspace">
      <WorkspaceNavigation administrator profile={profile} leave={signOut} back={back} />
      <main id="main-content" tabIndex={-1} className="workspace-main">
        <WorkspacePageHeader title={ar ? 'رسالة الدعوة الخاصة' : 'Private invitation message'} />
        <section className="panel space-y-3">
          {state.kind === 'ready' && (
            <button className="primary" onClick={() => void reveal()}>
              {ar ? 'إظهار الدعوة' : 'Reveal invitation'}
            </button>
          )}
          {state.kind === 'loading' && <p role="status">{ar ? 'جارٍ التحميل…' : 'Loading…'}</p>}
          {state.kind === 'unavailable' && (
            <p role="alert">
              {ar
                ? 'الرسالة غير متاحة أو انتهت صلاحيتها. تحقق من الحسابات وأعد تعيين كلمة المرور عند الحاجة.'
                : 'The message is unavailable or expired. Check accounts and reset the password if needed.'}
            </p>
          )}
          {state.kind === 'done' && (
            <>
              <p>
                {ar
                  ? 'أرسل كل رسالة إلى صاحب الحساب بشكل خاص. ستختفي الرسائل عند مغادرة الصفحة.'
                  : 'Send each message privately to its recipient. The messages disappear when you leave this page.'}
              </p>
              {state.invitations.map((invitation) => (
                <CopyableInvitation key={invitation.phone} invitation={invitation} ar={ar} />
              ))}
            </>
          )}
          <button className="secondary workspace-page-back" onClick={back.onClick}>
            {back.label}
          </button>
        </section>
      </main>
    </div>
  )
}
