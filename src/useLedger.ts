import { useLanguage } from './useLanguage'
import {
  useCallback,
  useEffect,
  useEffectEvent,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { z } from 'zod'
import {
  attendanceScopeKey,
  eventSchema,
  makeEvent,
  projectEvents,
  type LedgerEvent,
  type Payload,
} from '../shared/ledger'
import { assignedEvents, validateAssignedPayloads } from '../shared/roster'
import {
  apiUrl,
  getNotebook,
  getNotebookView,
  savesNetworkData,
  type NotebookFocus,
  type NotebookView,
  postEvents,
  postCorrections,
  NeonRequestError,
  InvalidNotebookSnapshotError,
} from './neon'
import {
  allowOfflineAccess,
  authorizeLocalGrant,
  clearLegacyOfflineAccess,
  clearOfflineAccess,
  localGrant,
  offlineAccess,
} from './offlineAccess'
import {
  cacheOfflineRoster,
  getOfflineSnapshot,
  listPendingAttendance,
  listPendingStudent,
  queueNotebookOperations,
  queueStudentOperation,
  subscribeOfflineQueue,
  snapshotKey,
} from './offlineAttendanceStore'
import { syncOfflineAttendance, syncOfflineStudent } from './offlineAttendanceSync'
import { offlineNotebook, overlayNotebook } from './offlineNotebookData'
import { offlineAttendanceRecordSchema } from '../shared/offlineAttendance'
import {
  offlineProfileOperationSchema,
  offlineStudentOperationSchema,
  profileFields,
} from '../shared/offlineStudent'
import { subscribeWorkspaceChanges } from './syncStream'
import type {
  Course,
  Enrollment,
  NeonTeacher,
  NeonRosterStudent,
  NeonSnapshot,
  teacherStudentUpdateSchema,
} from '../shared/neon'

export type SaveStudentProfile = (
  input: z.infer<typeof teacherStudentUpdateSchema>,
) => Promise<boolean>

const foreignPendingMessage =
  'A pending batch belongs to another account or an old link. Copy the pending batch from browser storage and contact your administrator to reconcile it before saving.'
const pendingLock = 'halaqa-pending'
const locksUnavailableMessage =
  'This browser cannot safely coordinate saves between tabs. Use a current browser with Web Locks support.'

function hasForeignPending(key: string) {
  return Object.keys(localStorage).some(
    (item) =>
      (item.startsWith('halaqa-pending-v1:') || item.startsWith('halaqa-pending-neon:')) &&
      item !== key,
  )
}

async function withPendingLocks<T>(key: string, run: () => Promise<T> | T): Promise<T> {
  return navigator.locks.request(pendingLock, { ifAvailable: true }, async (globalLock) => {
    if (!globalLock) throw new Error('Another tab is saving this account. Wait and try again.')
    return navigator.locks.request(key, { ifAvailable: true }, async (accountLock) => {
      if (!accountLock) throw new Error('Another tab is saving this account. Wait and try again.')
      return run()
    })
  })
}

export function useLedger(
  neon: { userId: string; teacher: NeonTeacher },
  startOffline = false,
  enabled = true,
  focus?: NotebookFocus,
) {
  const { t, language } = useLanguage()
  const neonUserId = neon.userId
  const [snapshotTeacher, setSnapshotTeacher] = useState<NeonTeacher>()
  const key = `halaqa-pending-neon:${apiUrl}:${snapshotTeacher?.id ?? neon.teacher.id}`
  const [events, setEvents] = useState<LedgerEvent[]>([])
  const [roster, setRoster] = useState<NeonRosterStudent[]>([])
  const [courses, setCourses] = useState<Course[]>([])
  const [enrollments, setEnrollments] = useState<Enrollment[]>([])
  const [pending, setPending] = useState<LedgerEvent[]>([])
  const [storageBlocked, setStorageBlocked] = useState(() =>
    !navigator.locks ? locksUnavailableMessage : '',
  )
  const [loaded, setLoaded] = useState(false)
  const [busy, setBusy] = useState(false)
  const locked = useRef(false)
  const [error, setError] = useState('')
  const [readError, setReadError] = useState('')
  const [message, setMessage] = useState('')
  const [offline, setOffline] = useState(startOffline)
  const [offlineExpired, setOfflineExpired] = useState(false)
  const [offlinePrepared, setOfflinePrepared] = useState(false)
  const [offlinePendingCount, setOfflinePendingCount] = useState(0)
  const [offlineNeedsReview, setOfflineNeedsReview] = useState(false)
  const confirmed = useRef<NotebookView | undefined>(undefined)
  const persisted = useRef<NeonSnapshot | undefined>(undefined)
  const confirmedFocusKey = useRef<string | undefined>(undefined)
  const completeRead = useRef<{ controller: AbortController; timer: number } | undefined>(undefined)
  const focusKey = focus ? JSON.stringify(focus) : undefined
  const requestedFocus = useRef({ key: focusKey, value: focus })
  const [focused, setFocused] = useState<{
    key: string
    value: NonNullable<NotebookView['focus']>
  }>()
  useLayoutEffect(() => {
    requestedFocus.current = { key: focusKey, value: focus }
  })
  const visibleScope = useRef<string | null>(null)
  const syncWriting = useRef(false)
  const active = useRef<object | null>(null)
  const connected = useRef(false)
  const readInFlight = useRef<Promise<boolean> | null>(null)
  const syncInFlight = useRef<Promise<boolean> | null>(null)
  const readEpoch = useRef(0)
  const pendingEpoch = useRef(0)
  const writeEpoch = useRef(0)
  const syncAgain = useRef(false)
  const offlineRef = useRef(startOffline)
  const [invalidSnapshot, setInvalidSnapshot] = useState(false)
  const invalid = 0
  const stopCompleteRead = useCallback(() => {
    if (!completeRead.current) return
    window.clearTimeout(completeRead.current.timer)
    completeRead.current.controller.abort()
    completeRead.current = undefined
  }, [])
  const applySnapshot = useCallback((snapshot: NeonSnapshot) => {
    setSnapshotTeacher(snapshot.teacher)
    setEvents(snapshot.events)
    setRoster(snapshot.students)
    setCourses(snapshot.courses)
    setEnrollments(snapshot.enrollments)
    setLoaded(true)
  }, [])
  useEffect(() => {
    try {
      const otherKey = hasForeignPending(key)
      const stored = localStorage.getItem(key)
      // Restore the external pending batch when this access session mounts.
      // oxlint-disable-next-line react/set-state-in-effect
      setPending(stored === null ? [] : z.array(eventSchema).min(1).parse(JSON.parse(stored)))
      setStorageBlocked(
        otherKey ? foreignPendingMessage : navigator.locks ? '' : locksUnavailableMessage,
      )
    } catch {
      setStorageBlocked(
        'Pending storage is unavailable or a saved batch is corrupted. Keep a copy of browser storage and ask the manager for help before saving.',
      )
    }
  }, [key])

  const loadOffline = useCallback(async () => {
    const session = active.current
    const key = snapshotKey(neonUserId)
    const lease = offlineAccess()
    const grant = localGrant(neonUserId)
    const epoch = ++pendingEpoch.current
    const hasAccess = grant
      ? grant.account.role !== 'student' && grant.account.canRead
      : lease?.userId === neonUserId && lease.teacher?.id === neon.teacher.id
    const [cached, attendance, students] = await Promise.all([
      hasAccess
        ? getOfflineSnapshot(neonUserId, key).then(
            (value) =>
              value ??
              (grant &&
              lease?.userId === neonUserId &&
              lease.teacher?.id === grant.account.id &&
              lease.teacher.role === grant.account.role &&
              lease.teacher.canWrite === grant.account.canWrite
                ? getOfflineSnapshot(neonUserId, neonUserId)
                : null),
          )
        : Promise.resolve(null),
      listPendingAttendance(neonUserId),
      listPendingStudent(neonUserId),
    ])
    if (!session || active.current !== session || epoch !== pendingEpoch.current) return false
    if (
      snapshotKey(neonUserId) !== key ||
      (!grant && JSON.stringify(offlineAccess()) !== JSON.stringify(lease))
    )
      return false
    setOfflinePendingCount(attendance.length + students.length)
    setOfflineNeedsReview([...attendance, ...students].some(({ outcome }) => outcome !== null))
    setOfflinePrepared(Boolean(cached))
    const base = visibleScope.current === key ? confirmed.current : undefined
    const currentFocus = base?.focus && confirmedFocusKey.current === requestedFocus.current.key
    if (
      cached &&
      (!base || persisted.current === base || (base.focus && offlineRef.current && !currentFocus))
    ) {
      const local = offlineNotebook(cached, lease?.teacher ?? neon.teacher, attendance, students)
      // Invitation profiles belong to the live response only.
      visibleScope.current = key
      setFocused(undefined)
      applySnapshot(base ? { ...local, teacher: base.teacher } : local)
    } else if (
      base &&
      (!base.focus || currentFocus) &&
      (!offlineRef.current || hasAccess) &&
      visibleScope.current === key
    ) {
      const focusKey = confirmedFocusKey.current
      if (base.focus && focusKey) setFocused({ key: focusKey, value: base.focus })
      applySnapshot(overlayNotebook(base, attendance, students))
    }
    return Boolean(cached || (base && (!base.focus || currentFocus)))
  }, [neonUserId, neon.teacher, applySnapshot])

  async function accept(
    snapshot: NotebookView,
    session: object,
    cacheKey: string,
    viewKey?: string,
  ) {
    const isCurrent = () =>
      active.current === session &&
      snapshotKey(neonUserId) === cacheKey &&
      (!snapshot.focus || requestedFocus.current.key === viewKey)
    if (!isCurrent()) return false
    if (
      !snapshot.teacher.canRead ||
      (snapshot.teacher.expiresAt && Date.parse(snapshot.teacher.expiresAt) <= Date.now())
    ) {
      clearOfflineAccess()
      confirmed.current = undefined
      connected.current = false
      setLoaded(false)
      setOfflinePrepared(false)
      setOfflineExpired(true)
      return false
    }
    const grant = localGrant(neonUserId)
    if (
      grant &&
      (grant.account.id !== snapshot.teacher.id ||
        grant.account.role !== snapshot.teacher.role ||
        grant.account.canWrite !== snapshot.teacher.canWrite ||
        grant.account.expiresAt !== snapshot.teacher.expiresAt)
    ) {
      authorizeLocalGrant(neonUserId, snapshot.teacher)
      cacheKey = snapshotKey(neonUserId)
    }
    confirmed.current = snapshot
    confirmedFocusKey.current = snapshot.focus ? viewKey : undefined
    visibleScope.current = cacheKey
    setInvalidSnapshot(false)
    connected.current = true
    offlineRef.current = false
    setOffline(false)
    setOfflineExpired(false)
    setReadError('')
    setFocused(snapshot.focus && viewKey ? { key: viewKey, value: snapshot.focus } : undefined)
    // Publish the authorized response even when browser storage is unavailable.
    try {
      const epoch = pendingEpoch.current
      const [attendance, students] = await Promise.all([
        listPendingAttendance(neonUserId),
        listPendingStudent(neonUserId),
      ])
      if (!isCurrent()) return false
      if (epoch === pendingEpoch.current)
        applySnapshot(overlayNotebook(snapshot, attendance, students))
    } catch {
      if (isCurrent()) applySnapshot(snapshot)
    }
    if (!isCurrent()) return false
    if (snapshot.focus) return true
    try {
      await cacheOfflineRoster(neonUserId, snapshot, cacheKey)
      if (!isCurrent()) return false
      persisted.current = snapshot
      if (snapshot.teacher.canWrite) {
        allowOfflineAccess(
          neonUserId,
          snapshot.teacher.name,
          snapshot.teacher.expiresAt ? Date.parse(snapshot.teacher.expiresAt) : null,
          snapshot.teacher,
        )
      } else clearLegacyOfflineAccess()
      await loadOffline()
    } catch {
      // Storage failure cannot hide a validated online response.
    }
    return isCurrent()
  }

  function readNotebook(): Promise<boolean> {
    if (readInFlight.current) return readInFlight.current
    const session = active.current
    if (!session) return Promise.resolve(false)
    const cacheKey = snapshotKey(neonUserId)
    const epoch = readEpoch.current
    const view = requestedFocus.current
    const isCurrent = () => active.current === session && snapshotKey(neonUserId) === cacheKey
    stopCompleteRead()
    const task = (async () => {
      try {
        const started = performance.now()
        const focus = confirmed.current && !confirmed.current.focus ? undefined : view.value
        const snapshot = await getNotebookView(neonUserId, focus)
        const rapid = performance.now() - started < 450
        if (!isCurrent() || locked.current || syncWriting.current || epoch !== readEpoch.current)
          return false
        const accepted = await accept(snapshot, session, cacheKey, view.key)
        if (
          accepted &&
          snapshot.focus &&
          rapid &&
          !(view.value?.kind === 'history' && view.value.before) &&
          !savesNetworkData() &&
          document.visibilityState === 'visible'
        ) {
          const controller = new AbortController()
          const stillCurrent = () =>
            !controller.signal.aborted &&
            isCurrent() &&
            confirmed.current === snapshot &&
            !locked.current &&
            !syncWriting.current &&
            epoch === readEpoch.current &&
            requestedFocus.current.key === view.key
          const timer = window.setTimeout(() => {
            if (!stillCurrent()) {
              if (completeRead.current?.controller === controller) completeRead.current = undefined
              return
            }
            void getNotebookView(neonUserId, undefined, controller.signal)
              .then((complete) => {
                if (stillCurrent()) return accept(complete, session, cacheKey)
              })
              .catch(() => {})
              .finally(() => {
                if (completeRead.current?.controller === controller)
                  completeRead.current = undefined
              })
          }, 750)
          completeRead.current = { controller, timer }
        }
        return accepted
      } catch (cause) {
        if (!isCurrent() || requestedFocus.current.key !== view.key) return false
        if (cause instanceof NeonRequestError && (cause.status === 401 || cause.status === 403)) {
          if (cause.status === 403) clearOfflineAccess()
          confirmed.current = undefined
          connected.current = false
          setLoaded(false)
          setOfflinePrepared(false)
          setReadError(errorText(cause))
        } else if (!(cause instanceof InvalidNotebookSnapshotError)) {
          connected.current = false
          offlineRef.current = true
          setOffline(true)
          if (await loadOffline().catch(() => false)) setReadError('')
          else setReadError(errorText(cause))
        } else {
          connected.current = false
          setInvalidSnapshot(true)
          setReadError(errorText(cause))
        }
        return false
      }
    })()
    readInFlight.current = task
    void task
      .finally(() => {
        if (readInFlight.current === task) readInFlight.current = null
      })
      .catch(() => {})
    return task
  }

  function drain(retryBlocked = false): Promise<boolean> {
    if (syncInFlight.current) {
      syncAgain.current = true
      return syncInFlight.current
    }
    const session = active.current
    if (!session || !connected.current || !navigator.locks || storageBlocked)
      return Promise.resolve(false)
    const cacheKey = snapshotKey(neonUserId)
    const isCurrent = () => active.current === session && snapshotKey(neonUserId) === cacheKey
    const task = (async () => {
      if (hasForeignPending(key) || localStorage.getItem(key)) return false
      const [attendance, students] = await Promise.all([
        listPendingAttendance(neonUserId),
        listPendingStudent(neonUserId),
      ])
      if (!isCurrent() || !connected.current || !confirmed.current?.teacher.canWrite) return false
      if (!attendance.length && !students.length) return true
      syncWriting.current = true
      readEpoch.current += 1
      const outcomes = await Promise.allSettled([
        syncOfflineAttendance(neonUserId, retryBlocked, cacheKey),
        syncOfflineStudent(neonUserId, retryBlocked, cacheKey),
      ])
      if (!isCurrent()) return false
      syncWriting.current = false
      const uploaded = outcomes.some(
        (result) =>
          result.status === 'fulfilled' &&
          result.value.some((outcome) => outcome.kind === 'saved' || outcome.kind === 'unchanged'),
      )
      const conflicted = outcomes.some(
        (result) =>
          result.status === 'fulfilled' && result.value.some(({ kind }) => kind === 'conflict'),
      )
      if (uploaded) {
        const savedWriteEpoch = writeEpoch.current
        const [remainingMarks, remainingStudents] = await Promise.all([
          listPendingAttendance(neonUserId),
          listPendingStudent(neonUserId),
        ])
        if (
          isCurrent() &&
          writeEpoch.current === savedWriteEpoch &&
          !remainingMarks.length &&
          !remainingStudents.length
        )
          setMessage(
            language === 'ar'
              ? '\u0642\u0628\u0644\u062a \u0648\u0627\u0631\u062a\u0642\u0650 \u0627\u0644\u062a\u063a\u064a\u064a\u0631\u0627\u062a.'
              : 'Changes accepted by Wartaq.',
          )
      }
      const denied = outcomes.some(
        (result) =>
          result.status === 'rejected' &&
          result.reason instanceof NeonRequestError &&
          (result.reason.status === 401 || result.reason.status === 403),
      )
      const refreshSnapshot = async () => {
        // A concurrent GET may have started before the commit. Follow it with a fresh read.
        const inFlight = readInFlight.current
        if (inFlight) await inFlight
        if (isCurrent()) await readNotebook()
      }
      if (denied) await refreshSnapshot()
      else if (uploaded || conflicted) void refreshSnapshot()
      return outcomes.every((result) => result.status === 'fulfilled')
    })().catch(() => false)
    syncInFlight.current = task
    void task.finally(() => {
      if (syncInFlight.current !== task) return
      syncInFlight.current = null
      syncWriting.current = false
      if (syncAgain.current && isCurrent()) {
        syncAgain.current = false
        void drain()
      }
    })
    return task
  }

  async function requestRefresh(retryBlocked = false) {
    if (!enabled) return false
    if (retryBlocked && syncInFlight.current) await syncInFlight.current
    if (!(await readNotebook())) return false
    const synced = await drain(retryBlocked)
    return !retryBlocked || synced
  }

  const refreshRef = useRef(requestRefresh)
  useLayoutEffect(() => {
    refreshRef.current = requestRefresh
  })
  const refresh = useCallback((retryBlocked = false) => refreshRef.current(retryBlocked), [])
  const previousFocus = useRef(focusKey)
  useEffect(() => {
    if (previousFocus.current === focusKey) return
    previousFocus.current = focusKey
    if (!enabled || (confirmed.current && !confirmed.current.focus)) return
    let cancelled = false
    void Promise.resolve(readInFlight.current).then(() => {
      if (!cancelled && (!confirmed.current || confirmed.current.focus)) void refresh()
    })
    return () => {
      cancelled = true
    }
  }, [enabled, focusKey, refresh])
  useEffect(() => {
    const visibility = () => {
      if (document.visibilityState !== 'visible') stopCompleteRead()
    }
    document.addEventListener('visibilitychange', visibility)
    return () => {
      stopCompleteRead()
      document.removeEventListener('visibilitychange', visibility)
    }
  }, [stopCompleteRead])
  const refreshAfterWrite = useCallback(async () => {
    const session = active.current
    const cacheKey = snapshotKey(neonUserId)
    // The in-flight snapshot may have started before the write committed.
    await readInFlight.current
    if (active.current !== session || snapshotKey(neonUserId) !== cacheKey) return false
    return refresh()
  }, [neonUserId, refresh])

  const onQueueChange = useEffectEvent(() => {
    void loadOffline()
      .then((prepared) => {
        if (!prepared && !navigator.onLine && active.current)
          setReadError('Offline access expired. Connect and sign in again.')
      })
      .catch(() => {})
  })
  const onProbe = useEffectEvent(async () => {
    await refresh()
  })
  const probeOfflineConnection = useEffectEvent(() => {
    if (offlineRef.current && document.visibilityState === 'visible') void refresh()
  })
  const onDisconnect = useEffectEvent(() => {
    connected.current = false
    offlineRef.current = true
    setOffline(true)
    void loadOffline().catch(() => {})
  })
  const checkOfflineAccess = useEffectEvent(() => {
    if (visibleScope.current && visibleScope.current !== snapshotKey(neonUserId)) {
      visibleScope.current = null
      confirmed.current = undefined
      persisted.current = undefined
      connected.current = false
      setLoaded(false)
      setOfflinePrepared(false)
      if (enabled && localGrant(neonUserId)) void refresh()
    }
    if (!offlineRef.current || localGrant(neonUserId) || offlineAccess()?.userId === neonUserId)
      return
    confirmed.current = undefined
    setOfflinePrepared(false)
    setOfflineExpired(true)
    setLoaded(false)
  })
  useEffect(() => {
    const session = {}
    active.current = session
    const unsubscribe = subscribeOfflineQueue(onQueueChange)
    queueMicrotask(() => {
      if (active.current === session) onQueueChange()
    })
    window.addEventListener('offline', onDisconnect)
    window.addEventListener('storage', checkOfflineAccess)
    const accessTimer = window.setInterval(checkOfflineAccess, 1000)
    return () => {
      active.current = null
      confirmed.current = undefined
      visibleScope.current = null
      readInFlight.current = null
      persisted.current = undefined
      connected.current = false
      syncAgain.current = false
      syncInFlight.current = null
      syncWriting.current = false
      unsubscribe()
      window.removeEventListener('offline', onDisconnect)
      window.removeEventListener('storage', checkOfflineAccess)
      window.clearInterval(accessTimer)
    }
  }, [neonUserId])
  useEffect(() => {
    if (!enabled) return
    const unsubscribe = subscribeWorkspaceChanges(neonUserId, onProbe)
    const retryTimer = window.setInterval(probeOfflineConnection, 5000)
    return () => {
      unsubscribe()
      window.clearInterval(retryTimer)
    }
  }, [enabled, neonUserId])

  async function send(batch: LedgerEvent[], retry = false): Promise<boolean> {
    if (locked.current || invalidSnapshot || !batch.length) return false
    const session = active.current
    const cacheKey = snapshotKey(neonUserId)
    const isCurrent = () => active.current === session && snapshotKey(neonUserId) === cacheKey
    if (!session) return false
    locked.current = true
    readEpoch.current += 1
    setBusy(true)
    setError('')
    setMessage('')
    let writeStarted = false
    try {
      if (!navigator.locks) {
        setStorageBlocked(
          'This browser cannot safely coordinate saves between tabs. Use a current browser with Web Locks support.',
        )
        return false
      }
      const saved = await withPendingLocks(key, async () => {
        if (!isCurrent()) return false
        let foreign: boolean
        let existing: string | null
        try {
          foreign = hasForeignPending(key)
          existing = localStorage.getItem(key)
        } catch {
          const reason =
            'Could not persist the pending batch. No write was sent; restore browser storage before saving.'
          setStorageBlocked(reason)
          throw new Error(reason)
        }
        if (foreign) {
          setStorageBlocked(foreignPendingMessage)
          throw new Error(foreignPendingMessage)
        }
        if (
          !retry &&
          ((await listPendingAttendance(neonUserId)).length ||
            (await listPendingStudent(neonUserId)).length)
        )
          throw new Error('Review pending offline changes before saving online.')
        const serialized = JSON.stringify(batch)
        if (retry ? existing !== serialized : existing !== null) {
          const reason = 'The pending batch changed in another tab. Reload before retrying.'
          setStorageBlocked(reason)
          throw new Error(reason)
        }
        if (!retry) {
          try {
            localStorage.setItem(key, serialized)
          } catch {
            const reason =
              'Could not persist the pending batch. No write was sent; restore browser storage before saving.'
            setStorageBlocked(reason)
            throw new Error(reason)
          }
        }
        setPending(batch)
        if (!neon.teacher.canWrite) throw new Error('Your account has read-only access.')
        if (confirmed.current?.focus && batch.every(({ payload }) => payload.kind === 'reversal')) {
          writeStarted = true
          await postCorrections(neonUserId, batch)
          if (!isCurrent()) return false
          localStorage.removeItem(key)
          return true
        }
        const current = await getNotebook(neon.userId)
        if (!isCurrent()) return false
        applySnapshot(current)
        if (
          projectEvents([...current.events, ...batch]).conflicts >
          projectEvents(current.events).conflicts
        )
          throw new Error(
            'A pending ID conflicts with a saved record. Ask the manager to inspect it before retrying.',
          )
        const ids = new Set(current.events.map((event) => event.id))
        const remaining = batch.filter((event) => !ids.has(event.id))
        validateAssignedPayloads(
          remaining.map((event) => event.payload),
          current.students,
          current.events,
          current.enrollments,
          current.courses,
        )
        let snapshot = current
        if (remaining.length) {
          writeStarted = true
          snapshot = await postEvents(neon.userId, remaining)
        }
        if (!isCurrent()) return false
        confirmed.current = snapshot
        confirmedFocusKey.current = undefined
        setFocused(undefined)
        applySnapshot(snapshot)
        if (localGrant(neonUserId) || offlineAccess()?.userId === neonUserId)
          await cacheOfflineRoster(neonUserId, snapshot, cacheKey)
            .then(() => {
              if (isCurrent()) persisted.current = snapshot
            })
            .catch(() => {})
        if (!isCurrent()) return false
        localStorage.removeItem(key)
        return true
      })
      if (!saved) return false
      setPending([])
      if (confirmed.current?.focus) queueMicrotask(() => void refreshAfterWrite())
      setMessage(`${batch.length} record${batch.length === 1 ? '' : 's'} saved.`)
      return true
    } catch (cause) {
      if (!isCurrent()) return false
      setError(
        `${errorText(cause)}${writeStarted ? ' The save may already have reached the server. Retry the same pending batch to avoid counting it twice.' : ''}`,
      )
      return false
    } finally {
      locked.current = false
      setBusy(false)
    }
  }

  async function save(payloads: Payload[], observedMark?: string | null): Promise<boolean> {
    if (storageBlocked || pending.length || !loaded || !payloads.length || locked.current)
      return false
    const session = active.current
    const cacheKey = snapshotKey(neonUserId)
    const access = snapshotTeacher ?? neon.teacher
    const isCurrent = () => active.current === session && snapshotKey(neonUserId) === cacheKey
    if (!session || !access.canWrite || visibleScope.current !== cacheKey) return false
    const queueable = payloads.every(
      (payload) =>
        payload.kind === 'note' || (payload.kind === 'attendance' && !!payload.enrollmentId),
    )
    try {
      if (!queueable) {
        if (offline) throw new Error('This action is unavailable offline.')
        return await send(payloads.map((payload) => makeEvent(neon.teacher.name, payload)))
      }
      const marks: z.infer<typeof offlineAttendanceRecordSchema>[] = []
      const notes: z.infer<typeof offlineStudentOperationSchema>[] = []
      const projection = projectEvents(events)
      validateAssignedPayloads(payloads, roster, events, enrollments, courses)
      for (const payload of payloads) {
        if (payload.kind === 'attendance') {
          const current = projection.attendanceVersions.get(attendanceScopeKey(payload))
          const event = makeEvent(neon.teacher.name, payload)
          marks.push(
            offlineAttendanceRecordSchema.parse({
              event,
              editTime: event.editTime,
              expectedEventId:
                observedMark === undefined ? (current?.event.id ?? null) : observedMark,
            }),
          )
        } else {
          notes.push(
            offlineStudentOperationSchema.parse({
              kind: 'note',
              event: makeEvent(neon.teacher.name, payload),
            }),
          )
        }
      }
      setError('')
      await withPendingLocks(key, async () => {
        if (!isCurrent() || (!localGrant(neonUserId) && offlineAccess()?.userId !== neonUserId))
          throw new Error('Offline access has expired or been withdrawn.')
        if (hasForeignPending(key)) {
          setStorageBlocked(foreignPendingMessage)
          throw new Error(foreignPendingMessage)
        }
        if (localStorage.getItem(key))
          throw new Error('Resolve the pending online batch before saving offline.')
        try {
          await queueNotebookOperations(
            neonUserId,
            marks,
            notes,
            observedMark === undefined && !confirmed.current?.focus ? cacheKey : undefined,
          )
        } catch {
          throw new Error(
            'Could not save these changes on this device. Restore browser storage and try again.',
          )
        }
      })
      if (!isCurrent()) return true
      writeEpoch.current += 1
      setMessage(
        language === 'ar'
          ? '\u062d\u064f\u0641\u0638\u062a \u0639\u0644\u0649 \u0647\u0630\u0627 \u0627\u0644\u062c\u0647\u0627\u0632. \u0628\u0627\u0646\u062a\u0638\u0627\u0631 \u0627\u0644\u0645\u0632\u0627\u0645\u0646\u0629.'
          : 'Saved on this device. Waiting to sync.',
      )
      await loadOffline().catch(() => false)
      void drain()
      return true
    } catch (cause) {
      if (isCurrent()) setError(errorText(cause))
      return false
    }
  }
  async function discard() {
    if (
      !window.confirm(
        t(
          'Stop retrying this batch? It may already exist on the server. Refresh and check the saved records before entering it again.',
        ),
      )
    )
      return
    if (!navigator.locks) {
      setStorageBlocked(
        'This browser cannot safely coordinate saves between tabs. Use a current browser with Web Locks support.',
      )
      return
    }
    try {
      await withPendingLocks(key, () => {
        if (localStorage.getItem(key) !== JSON.stringify(pending))
          throw new Error('The pending batch changed in another tab. Reload before discarding.')
        localStorage.removeItem(key)
        setPending([])
        setError('')
      })
    } catch (cause) {
      setError(errorText(cause))
    }
  }

  const saveStudentProfile: SaveStudentProfile = async (input) => {
    const session = active.current
    const cacheKey = snapshotKey(neonUserId)
    const isCurrent = () => active.current === session && snapshotKey(neonUserId) === cacheKey
    if (
      !session ||
      visibleScope.current !== cacheKey ||
      !(snapshotTeacher ?? neon.teacher).canWrite ||
      storageBlocked ||
      invalidSnapshot ||
      pending.length ||
      !roster.some((student) => student.studentId === input.studentId && student.active)
    )
      return false
    const changed = profileFields.filter((field) => input[field] !== input.expected[field])
    if (!changed.length) return true
    const operation = offlineProfileOperationSchema.parse({
      kind: 'profile-v1',
      editTime: new Date().toISOString(),
      id: crypto.randomUUID(),
      studentId: input.studentId,
      clientTime: new Date().toISOString(),
      expected: Object.fromEntries(changed.map((field) => [field, input.expected[field]])),
      changes: Object.fromEntries(changed.map((field) => [field, input[field]])),
    })
    try {
      if (
        offline &&
        changed.some((field) => field !== 'whatsapp') &&
        !(await getOfflineSnapshot(neonUserId, cacheKey))?.notebook
      )
        throw new Error(
          'Connect to Wartaq once to prepare this student profile for offline editing.',
        )
      await withPendingLocks(key, async () => {
        if (!isCurrent() || (!localGrant(neonUserId) && offlineAccess()?.userId !== neonUserId))
          throw new Error('Offline access has expired or been withdrawn.')
        if (hasForeignPending(key) || localStorage.getItem(key))
          throw new Error(foreignPendingMessage)
        await queueStudentOperation(neonUserId, operation)
      })
      if (!isCurrent()) return true
      writeEpoch.current += 1
      setError('')
      setMessage(
        language === 'ar'
          ? '\u062d\u064f\u0641\u0638\u062a \u0639\u0644\u0649 \u0647\u0630\u0627 \u0627\u0644\u062c\u0647\u0627\u0632. \u0628\u0627\u0646\u062a\u0638\u0627\u0631 \u0627\u0644\u0645\u0632\u0627\u0645\u0646\u0629.'
          : 'Saved on this device. Waiting to sync.',
      )
      await loadOffline().catch(() => false)
      void drain()
      return true
    } catch (cause) {
      if (isCurrent()) setError(errorText(cause))
      return false
    }
  }

  const historyPage = focused?.value.kind === 'history'
  const scopedEvents = useMemo(
    () => (historyPage ? events : assignedEvents(events, roster)),
    [events, historyPage, roster],
  )
  return {
    events: scopedEvents,
    roster,
    courses,
    enrollments,
    snapshotTeacher,
    pending,
    storageBlocked,
    loaded: loaded && (!focused || focused.key === focusKey),
    focus: focused?.key === focusKey ? focused?.value : undefined,
    busy,
    error: readError || error,
    message,
    invalid,
    invalidSnapshot,
    offline,
    offlineExpired,
    offlinePrepared,
    offlinePendingCount,
    offlineNeedsReview,
    saveStudentProfile,
    refresh,
    refreshAfterWrite,
    save,
    retry: () => (storageBlocked ? Promise.resolve(false) : send(pending, true)),
    discard,
  }
}
function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : 'The request failed.'
}
