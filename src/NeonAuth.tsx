import { LanguageSwitch } from './i18n'
import { useLanguage } from './useLanguage'
import { useEffect, useEffectEvent, useMemo, useRef, useState } from 'react'
import type { NeonAccount } from '../shared/neon'
import type {
  invitationProfileInputSchema,
  invitationSetupInputSchema,
} from '../shared/invitations'
import { z } from 'zod'
import {
  AccessDeniedEvent,
  authClient,
  getAccount,
  NeonRequestError,
  request,
  announceSessionChange,
  resetWorkspace,
  sessionChangeEvent,
  sessionChangeSource,
} from './neon'
import { InvitationSetupForm } from './InvitationProfile'
import { WorkspaceLoading } from './WorkspaceLoading'
import type { Profile } from './ProfileMenu'
import { WartaqLogo } from './brand/components'
import { AccountChoices } from './AccountChoices'
import { flushDiagnostics, recordDiagnosticAction } from './diagnostics'
import {
  authorizeLocalGrant,
  clearOfflineAccess,
  grantStorageKey,
  localGrant,
  offlineAccess,
  type CachedAccess,
} from './offlineAccess'
import { subscribeWorkspaceChanges } from './syncStream'
import './auth.css'

const deviceLimitMessage = 'Five accounts are signed in. Sign out of one to add another.'

function AuthHeader() {
  const { language } = useLanguage()
  return (
    <header className="workspace-auth-header">
      <a
        href={window.location.pathname}
        aria-label={language === 'ar' ? 'الصفحة الرئيسية' : 'Home'}
      >
        <WartaqLogo />
      </a>
      <LanguageSwitch />
    </header>
  )
}

function cachedAccount(): { userId: string; value: NeonAccount } | null {
  const grant = localGrant()
  if (grant) return { userId: grant.userId, value: grant.account }
  const legacy = offlineAccess()
  if (!legacy) return null
  return {
    userId: legacy.userId,
    value: legacy.teacher ?? {
      role: 'teacher',
      id: '00000000-0000-0000-0000-000000000000',
      name: legacy.actor,
      canRead: true,
      canWrite: true,
      expiresAt: legacy.expires === null ? null : new Date(legacy.expires).toISOString(),
    },
  }
}

export function NeonAuth({
  children,
  onSignOut,
}: {
  onSignOut: () => void
  children: (
    userId: string,
    teacher: NeonAccount,
    signOut: () => void,
    profile: Profile,
    cached?: CachedAccess,
  ) => React.ReactNode
}) {
  const { t, errorMessage, language } = useLanguage()
  const session = authClient.useSession()
  const userId = session.data?.user.id
  const sessionDenied = session.error?.status === 401 || session.error?.status === 403
  const sessionMissing = !session.isPending && !session.error && !userId
  const [offlineSession, setOfflineSession] = useState(cachedAccount)
  const [connectionLost, setConnectionLost] = useState(false)
  const validation = useRef(0)
  const [account, setAccount] = useState<
    | { kind: 'ready'; userId: string; value: NeonAccount }
    | { kind: 'denied' | 'error'; userId: string; message: string }
    | null
  >(null)
  useEffect(() => {
    const cached = cachedAccount()
    if (sessionDenied || sessionMissing || (cached && userId && cached.userId !== userId)) {
      clearOfflineAccess()
      // oxlint-disable-next-line react/set-state-in-effect
      setOfflineSession(null)
      setAccount(null)
    }
  }, [sessionDenied, sessionMissing, userId])
  useEffect(() => {
    const disconnect = () => {
      const cached = cachedAccount()
      if (cached && (!userId || cached.userId === userId)) {
        setOfflineSession(cached)
        setConnectionLost(true)
      }
    }
    window.addEventListener('offline', disconnect)
    return () => window.removeEventListener('offline', disconnect)
  }, [userId])
  const [logout, setLogout] = useState<'idle' | 'pending' | 'failed'>('idle')
  const [accountAttempt, setAccountAttempt] = useState(0)
  const [addingAccount, setAddingAccount] = useState(false)
  const [choosingAccount, setChoosingAccount] = useState(false)
  const addAccount = () => {
    clearOfflineAccess()
    setOfflineSession(null)
    setError('')
    setAddingAccount(true)
    setChoosingAccount(false)
  }
  useEffect(() => {
    let leaving = false
    const refreshWorkspace = () => {
      if (leaving) return
      leaving = true
      window.location.replace(window.location.pathname + '?page=app')
    }
    const changed = (event: StorageEvent) => {
      if (event.key === sessionChangeEvent) refreshWorkspace()
      if (event.key === grantStorageKey && event.oldValue && !event.newValue) {
        validation.current += 1
        setOfflineSession(null)
        setAccount(null)
        setAccountAttempt((value) => value + 1)
      }
    }
    const restored = (event: PageTransitionEvent) => {
      if (event.persisted) window.location.reload()
    }
    window.addEventListener('pageshow', restored)
    window.addEventListener('storage', changed)
    let channel: BroadcastChannel | undefined
    try {
      channel = new BroadcastChannel(sessionChangeEvent)
      channel.onmessage = (event: MessageEvent<unknown>) => {
        if (event.data !== sessionChangeSource) refreshWorkspace()
      }
    } catch {
      /* BroadcastChannel can be unavailable. */
    }
    return () => {
      window.removeEventListener('storage', changed)
      window.removeEventListener('pageshow', restored)
      channel?.close()
    }
  }, [])
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [stalled, setStalled] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const [retrying, setRetrying] = useState(false)
  const retryGeneration = useRef(0)
  const [reconnectRequested, setReconnectRequested] = useState(false)
  const offline =
    connectionLost || !navigator.onLine || Boolean(session.error) || account?.kind === 'error'
  const cachedActions = useMemo(
    () => ({ offline, reconnect: () => setReconnectRequested(true) }),
    [offline],
  )
  const refreshSession = useEffectEvent(() =>
    session
      .refetch()
      .catch(() => {})
      .finally(() => {
        setAccountAttempt((value) => value + 1)
        setReconnectRequested(false)
      }),
  )
  useEffect(() => {
    if (reconnectRequested) void refreshSession()
  }, [reconnectRequested])
  const reconnectUser = !userId && !sessionDenied && !sessionMissing ? offlineSession?.userId : null
  const repairSession = useEffectEvent(async () => {
    if (session.error && !session.isRefetching) await refreshSession()
  })
  useEffect(() => {
    if (!reconnectUser) return
    return subscribeWorkspaceChanges(reconnectUser, repairSession)
  }, [reconnectUser])
  useEffect(() => {
    if (!session.isPending && !retrying) return
    const timer = window.setTimeout(() => setStalled(true), 10000)
    return () => window.clearTimeout(timer)
  }, [session.isPending, retrying, attempt])
  if (stalled && !session.isPending && !retrying) setStalled(false)
  useEffect(() => {
    if (!userId || logout !== 'idle' || addingAccount || sessionDenied) return
    let cancelled = false
    const refresh = async () => {
      const attempt = ++validation.current
      try {
        const value = await getAccount(userId)
        if (cancelled || attempt !== validation.current) return
        if (value.expiresAt !== null && Date.parse(value.expiresAt) <= Date.now())
          throw new NeonRequestError(403)
        authorizeLocalGrant(userId, value)
        setOfflineSession(cachedAccount())
        setConnectionLost(false)
        setAccount({ kind: 'ready', userId, value })
      } catch (cause: unknown) {
        if (cancelled || attempt !== validation.current) return
        const denied =
          cause instanceof NeonRequestError && (cause.status === 401 || cause.status === 403)
        if (denied) {
          if (cachedAccount()?.userId === userId) clearOfflineAccess()
          setOfflineSession(null)
        }
        setAccount({
          userId,
          kind: denied ? 'denied' : 'error',
          message:
            cause instanceof NeonRequestError ? cause.message : new NeonRequestError(500).message,
        })
      }
    }
    if (!navigator.onLine) void refresh()
    const unsubscribe = subscribeWorkspaceChanges(userId, refresh)
    return () => {
      cancelled = true
      unsubscribe()
    }
  }, [userId, accountAttempt, logout, addingAccount, sessionDenied])
  useEffect(() => {
    const denied = (event: Event) => {
      if (
        event instanceof AccessDeniedEvent &&
        (event.userId === userId || (!userId && event.userId === offlineSession?.userId))
      ) {
        validation.current += 1
        if (cachedAccount()?.userId === event.userId) clearOfflineAccess()
        setOfflineSession(null)
        setAccount({
          userId: event.userId,
          kind: 'denied',
          message: new NeonRequestError(event.status).message,
        })
      }
    }
    window.addEventListener(AccessDeniedEvent.eventName, denied)
    return () => window.removeEventListener(AccessDeniedEvent.eventName, denied)
  }, [offlineSession?.userId, userId])
  const expiresAt =
    account?.kind === 'ready' && account.userId === userId
      ? account.value.expiresAt
      : offlineSession?.value.expiresAt
  useEffect(() => {
    if (!expiresAt) return
    const expiration = Date.parse(expiresAt)
    let timeout: number
    const expire = () => {
      const remaining = expiration - Date.now()
      if (remaining > 0) {
        timeout = window.setTimeout(expire, Math.min(remaining, 2147483647))
        return
      }
      validation.current += 1
      clearOfflineAccess()
      setOfflineSession(null)
      const id = userId ?? offlineSession?.userId
      if (id) setAccount({ kind: 'denied', userId: id, message: new NeonRequestError(403).message })
    }
    timeout = window.setTimeout(expire, Math.max(0, Math.min(expiration - Date.now(), 2147483647)))
    window.addEventListener('focus', expireOnFocus)
    function expireOnFocus() {
      if (expiration <= Date.now()) {
        window.clearTimeout(timeout)
        expire()
      }
    }
    return () => {
      window.clearTimeout(timeout)
      window.removeEventListener('focus', expireOnFocus)
    }
  }, [expiresAt, offlineSession?.userId, userId])
  const signOut = async () => {
    if (userId) {
      recordDiagnosticAction(userId, 'sign-out', 'activated')
      flushDiagnostics(userId)
    }
    clearOfflineAccess()
    setOfflineSession(null)
    setLogout('pending')
    try {
      const result = await authClient.signOut({
        fetchOptions: {
          signal: AbortSignal.timeout(15000),
          headers: { 'X-User-Id': userId ?? offlineSession?.userId ?? '' },
        },
      })
      if (result.error) {
        setLogout('failed')
        return
      }
      clearOfflineAccess()
      announceSessionChange()
      onSignOut()
    } catch {
      setLogout('failed')
    }
  }
  if (
    logout === 'idle' &&
    !addingAccount &&
    !choosingAccount &&
    !sessionDenied &&
    !sessionMissing &&
    offlineSession &&
    (!userId || userId === offlineSession.userId) &&
    account?.kind !== 'denied' &&
    (account?.kind !== 'ready' || Boolean(session.error) || connectionLost)
  ) {
    return children(
      offlineSession.userId,
      offlineSession.value,
      signOut,
      {
        name: offlineSession.value.name,
        image: null,
        update: async () => {
          throw new Error('Connect to Wartaq to edit your profile.')
        },
      },
      cachedActions,
    )
  }
  if (logout === 'pending') return <WorkspaceLoading message={t('Signing out…')} />
  if (logout === 'failed')
    return (
      <main id="main-content" tabIndex={-1} className="workspace-auth panel space-y-3">
        <p role="alert">{t('Could not confirm sign-out. Please try again.')}</p>
        <button className="secondary" onClick={signOut}>
          {t('Sign out')}
        </button>
      </main>
    )
  if ((session.isPending || retrying) && !stalled)
    return <WorkspaceLoading message={t('Loading your session…')} />
  if (session.error || stalled)
    return (
      <main id="main-content" tabIndex={-1} className="workspace-auth panel space-y-3">
        <AuthHeader />
        <p role="alert">{t('Could not check your session. Please try again.')}</p>
        <button
          className="secondary"
          onClick={() => {
            const generation = ++retryGeneration.current
            setStalled(false)
            setRetrying(true)
            setAttempt((value) => value + 1)
            void session
              .refetch()
              .catch(() => {})
              .finally(() => {
                if (retryGeneration.current === generation) setRetrying(false)
              })
          }}
        >
          {t('Try again')}
        </button>
      </main>
    )
  if (userId && !addingAccount) {
    if (choosingAccount)
      return (
        <main id="main-content" tabIndex={-1} className="workspace-auth panel space-y-3">
          <AuthHeader />
          <AccountChoices currentUserId={userId} add={addAccount} />
          <button type="button" className="secondary" onClick={() => setChoosingAccount(false)}>
            {t('Cancel')}
          </button>
        </main>
      )
    if (account?.userId === userId && account.kind !== 'ready')
      return (
        <main id="main-content" tabIndex={-1} className="workspace-auth panel space-y-3">
          <AuthHeader />
          <p role="alert">{errorMessage(account.message)}</p>
          {account.kind === 'error' && (
            <button
              className="secondary"
              onClick={() => {
                setAccount(null)
                setAccountAttempt((value) => value + 1)
              }}
            >
              {t('Try again')}
            </button>
          )}
          <button className="secondary" onClick={() => setChoosingAccount(true)}>
            {language === 'ar' ? 'تبديل الحساب' : 'Switch account'}
          </button>
          <button className="secondary" onClick={signOut}>
            {t('Sign out')}
          </button>
        </main>
      )
    if (!account || account.userId !== userId || account.kind !== 'ready')
      return <WorkspaceLoading message={t('Checking your access…')} />
    const invitation = account.value.invitationProfile
    const updateInvitation = async (
      details:
        z.input<typeof invitationProfileInputSchema> | z.input<typeof invitationSetupInputSchema>,
    ) => {
      await request('/api/me/invitation-profile', userId, details, false)
      setAccountAttempt((value) => value + 1)
      if (details.newPassword) announceSessionChange()
    }
    if (invitation?.requiresSetup)
      return (
        <main id="main-content" tabIndex={-1} className="workspace-auth">
          <AuthHeader />
          <InvitationSetupForm
            key={userId}
            invitation={invitation}
            complete={(newPassword) => updateInvitation({ newPassword })}
            signOut={signOut}
          />
          <button className="secondary" onClick={() => setChoosingAccount(true)}>
            {language === 'ar' ? 'تبديل الحساب' : 'Switch account'}
          </button>
        </main>
      )
    if (!account.value.canRead)
      return (
        <main id="main-content" tabIndex={-1} className="workspace-auth panel space-y-3">
          <AuthHeader />
          <p>
            {t(
              'Your account has write-only access. Contact your manager for permission to view records.',
            )}
          </p>
          <button className="secondary" onClick={() => setChoosingAccount(true)}>
            {language === 'ar' ? 'تبديل الحساب' : 'Switch account'}
          </button>
          <button className="secondary" onClick={signOut}>
            {t('Sign out')}
          </button>
        </main>
      )
    return children(userId, account.value, signOut, {
      invitation,
      accounts: { currentUserId: userId, add: addAccount },
      updateInvitation: invitation ? updateInvitation : undefined,
      name: session.data?.user.name ?? '',
      image: session.data?.user.image ?? null,
      update: async (name, image) => {
        const result = await authClient.updateUser(
          { name, ...(image === undefined ? {} : { image }) },
          { headers: { 'X-User-Id': userId } },
        )
        recordDiagnosticAction(userId, 'profile', result.error ? 'failed' : 'saved')
        if (result.error) throw new Error('Could not save profile')
        await session.refetch()
      },
    })
  }
  return (
    <main id="main-content" tabIndex={-1} className="workspace-auth auth-welcome">
      <AuthHeader />
      {!addingAccount && <AccountChoices add={addAccount} />}
      {addingAccount && (
        <button
          type="button"
          className="secondary"
          disabled={busy}
          onClick={() => {
            setAddingAccount(false)
            setError('')
          }}
        >
          {t('Cancel')}
        </button>
      )}
      <form
        key={addingAccount ? 'add-account' : 'sign-in'}
        id="sign-in-form"
        method="post"
        aria-labelledby="sign-in-title"
        aria-busy={busy}
        className="panel space-y-3 auth-form"
        onSubmit={async (event) => {
          event.preventDefault()
          if (session.isPending || session.error || busy) return
          const form = event.currentTarget
          const credentials = new FormData(form)
          const phone = credentials.get('username')
          const password = credentials.get('password')
          if (typeof phone !== 'string' || typeof password !== 'string') return
          setBusy(true)
          setError('')
          try {
            const response = await fetch('/api/auth/sign-in/phone', {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ phone, password }),
            })
            if (!response.ok) {
              setError(
                response.status === 409 ? deviceLimitMessage : 'Invalid phone number or password.',
              )
              return
            }
            form.reset()
            if (addingAccount) {
              resetWorkspace()
            } else {
              announceSessionChange()
              await session.refetch()
            }
          } catch (cause) {
            const failure = z.object({ error: z.object({ status: z.number() }) }).safeParse(cause)
            setError(
              failure.success && failure.data.error.status === 401
                ? 'Invalid phone number or password.'
                : 'The authentication service is unavailable. Please try again.',
            )
          } finally {
            setBusy(false)
          }
        }}
      >
        <h1 id="sign-in-title" className="title">
          {addingAccount ? t('Sign in to another account') : t('Sign in')}
        </h1>
        <label htmlFor="sign-in-phone">
          {language === 'ar' ? 'رقم الهاتف' : 'Phone number'}
          <input
            id="sign-in-phone"
            name="username"
            required
            type="tel"
            autoComplete="username"
            enterKeyHint="next"
            dir="ltr"
            placeholder="09xx xxx xxx / +963 9xx xxx xxx"
            aria-describedby="phone-hint"
          />
        </label>
        <span id="phone-hint">
          {language === 'ar'
            ? 'يمكن أن يبدأ الرقم السوري بـ 09 أو +9639.'
            : 'Syrian numbers can start with 09 or +9639.'}
        </span>
        <label htmlFor="current-password">
          {t('Password')}
          <input
            id="current-password"
            name="password"
            required
            type="password"
            dir="ltr"
            enterKeyHint="go"
            autoComplete="current-password"
          />
        </label>
        {error && (
          <p role="alert">
            {error === 'Invalid phone number or password.'
              ? language === 'ar'
                ? 'رقم الهاتف أو كلمة المرور غير صحيحة.'
                : error
              : errorMessage(error)}
          </p>
        )}
        <button
          type="submit"
          className="primary auth-sign-in"
          disabled={busy || session.isPending || Boolean(session.error)}
        >
          {busy ? t('Please wait…') : t('Sign in')}
        </button>
        <p className="muted">
          {language === 'ar'
            ? 'تواصل مع المسؤول لإعادة تعيين كلمة المرور.'
            : 'Contact your administrator to reset your password.'}
        </p>
        <p className="muted">
          {language === 'ar'
            ? 'يحتفظ وارتق بتشخيصات الاستخدام لمدة 30 يومًا.'
            : 'Wartaq keeps usage diagnostics for 30 days.'}
        </p>
      </form>
    </main>
  )
}
