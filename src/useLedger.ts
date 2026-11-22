import { useLanguage } from './useLanguage'
import { useCallback, useEffect, useEffectEvent, useMemo, useRef, useState } from 'react'
import { z } from 'zod'
import {
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
  postEvents,
  NeonRequestError,
  InvalidNotebookSnapshotError,
} from './neon'
import { allowOfflineAccess, clearOfflineAccess, offlineAccess } from './offlineAccess'
import {
  cacheOfflineRoster,
  getOfflineSnapshot,
  listPendingAttendance,
  listPendingStudent,
  queueNotebookOperations,
  queueStudentOperation,
  subscribeOfflineQueue,
} from './offlineAttendanceStore'
import { syncOfflineAttendance, syncOfflineStudent } from './offlineAttendanceSync'
import { offlineNotebook } from './offlineNotebookData'
import { offlineAttendanceRecordSchema } from '../shared/offlineAttendance'
import { offlineStudentOperationSchema } from '../shared/offlineStudent'
import type {
  Course,
  Enrollment,
  NeonTeacher,
  NeonRosterStudent,
  NeonSnapshot,
} from '../shared/neon'

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

export function useLedger(neon: { userId: string; teacher: NeonTeacher }, startOffline = false) {
  const { t } = useLanguage()
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
  const [message, setMessage] = useState('')
  const [offline, setOffline] = useState(startOffline)
  const [offlineExpired, setOfflineExpired] = useState(false)
  const [offlinePrepared, setOfflinePrepared] = useState(false)
  const [offlinePendingCount, setOfflinePendingCount] = useState(0)
  const offlineRef = useRef(startOffline)
  const invalid = 0
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
    const access = offlineAccess()
    if (!access || access.userId !== neonUserId || !navigator.locks) {
      setOfflinePrepared(false)
      if (offlineRef.current) {
        setOfflineExpired(true)
        setLoaded(false)
      }
      return false
    }
    const [snapshot, attendance, students] = await Promise.all([
      getOfflineSnapshot(neonUserId),
      listPendingAttendance(neonUserId),
      listPendingStudent(neonUserId),
    ])
    // The lease may have expired or been withdrawn while IndexedDB was read.
    if (JSON.stringify(offlineAccess()) !== JSON.stringify(access)) {
      setOfflinePrepared(false)
      if (offlineRef.current) {
        setOfflineExpired(true)
        setLoaded(false)
      }
      return false
    }
    setOfflinePendingCount(attendance.length + students.length)
    setOfflinePrepared(Boolean(snapshot || attendance.length || students.length))
    if (!snapshot) {
      if (offlineRef.current) setLoaded(false)
      return false
    }
    setOfflineExpired(false)
    if (offlineRef.current)
      applySnapshot(offlineNotebook(snapshot, access.teacher ?? neon.teacher, attendance, students))
    return true
  }, [neonUserId, neon.teacher, applySnapshot])

  const refresh = useCallback(
    async (retryBlocked = false) => {
      if (locked.current) return false
      locked.current = true
      setBusy(true)
      const previousLease = JSON.stringify(offlineAccess())
      let snapshot: NeonSnapshot
      try {
        snapshot = await getNotebook(neonUserId)
      } catch (cause) {
        if (cause instanceof NeonRequestError && (cause.status === 401 || cause.status === 403)) {
          if (cause.status === 403) clearOfflineAccess()
          setLoaded(false)
          setOfflinePrepared(false)
          setError(errorText(cause))
        } else if (cause instanceof InvalidNotebookSnapshotError) {
          setError(errorText(cause))
        } else {
          if (offlineAccess()?.userId === neonUserId || !navigator.onLine) {
            offlineRef.current = true
            setOffline(true)
          }
          try {
            if (offlineRef.current && (await loadOffline())) setError('')
            else setError(errorText(cause))
          } catch {
            setLoaded(false)
            setError(errorText(cause))
          }
        }
        locked.current = false
        setBusy(false)
        return false
      }
      try {
        if (snapshot.teacher.expiresAt && Date.parse(snapshot.teacher.expiresAt) <= Date.now()) {
          clearOfflineAccess()
          setOfflineExpired(true)
          setLoaded(false)
          return false
        }
        offlineRef.current = false
        setOffline(false)
        setOfflineExpired(false)
        applySnapshot(snapshot)
        setError('')
        if (!snapshot.teacher.canRead || !snapshot.teacher.canWrite) {
          clearOfflineAccess()
          setOfflinePrepared(false)
          return true
        }
        // Local cache and sync are optional: failures must not hide authorized online data.
        try {
          await cacheOfflineRoster(neonUserId, snapshot)
          if (JSON.stringify(offlineAccess()) !== previousLease) return true
          allowOfflineAccess(
            neonUserId,
            snapshot.teacher.name,
            snapshot.teacher.expiresAt ? Date.parse(snapshot.teacher.expiresAt) : null,
            snapshot.teacher,
          )
          setOfflinePrepared(true)
          const [attendance, students] = await Promise.all([
            listPendingAttendance(neonUserId),
            listPendingStudent(neonUserId),
          ])
          setOfflinePendingCount(attendance.length + students.length)
          if (
            (attendance.length || students.length) &&
            navigator.locks &&
            !pending.length &&
            !storageBlocked
          ) {
            const sync = await withPendingLocks(key, async () => {
              if (hasForeignPending(key) || localStorage.getItem(key))
                return { uploaded: false, failed: true }
              const outcomes = await Promise.allSettled([
                syncOfflineAttendance(neonUserId, retryBlocked),
                syncOfflineStudent(neonUserId, retryBlocked),
              ])
              return {
                uploaded: outcomes.some(
                  (result) =>
                    result.status === 'fulfilled' &&
                    result.value.some((item) => item.kind === 'saved' || item.kind === 'unchanged'),
                ),
                failed: outcomes.some((result) => result.status === 'rejected'),
              }
            })
            if (sync.uploaded) {
              snapshot = await getNotebook(neonUserId)
              applySnapshot(snapshot)
              await cacheOfflineRoster(neonUserId, snapshot)
            }
            if (retryBlocked && sync.failed) return false
          }
        } catch {
          if (retryBlocked) return false
          /* Keep the live snapshot and pending work for later. */
        }
        return true
      } finally {
        locked.current = false
        setBusy(false)
      }
    },
    [neonUserId, applySnapshot, loadOffline, pending.length, storageBlocked, key],
  )

  const onQueueChange = useEffectEvent(() => {
    void loadOffline().catch(() => {
      if (offlineRef.current) setLoaded(false)
    })
  })
  const onProbe = useEffectEvent(() => {
    void refresh()
  })
  const onDisconnect = useEffectEvent(() => {
    offlineRef.current = true
    setOffline(true)
    void loadOffline().catch(() => setLoaded(false))
  })
  const checkOfflineAccess = useEffectEvent(() => {
    if (!offlineRef.current || offlineAccess()?.userId === neonUserId) return
    setOfflinePrepared(false)
    setOfflineExpired(true)
    setLoaded(false)
  })
  const retryOfflineConnection = useEffectEvent(() => {
    if (offlineRef.current) void refresh()
  })
  useEffect(() => {
    const unsubscribe = subscribeOfflineQueue(onQueueChange)
    if (startOffline) queueMicrotask(onDisconnect)
    queueMicrotask(onProbe)
    window.addEventListener('online', onProbe)
    window.addEventListener('offline', onDisconnect)
    window.addEventListener('focus', onProbe)
    window.addEventListener('storage', onQueueChange)
    const accessTimer = window.setInterval(checkOfflineAccess, 1000)
    const retryTimer = window.setInterval(retryOfflineConnection, 5000)
    return () => {
      unsubscribe()
      window.removeEventListener('online', onProbe)
      window.removeEventListener('offline', onDisconnect)
      window.removeEventListener('focus', onProbe)
      window.removeEventListener('storage', onQueueChange)
      window.clearInterval(accessTimer)
      window.clearInterval(retryTimer)
    }
  }, [startOffline])

  async function send(batch: LedgerEvent[], retry = false): Promise<boolean> {
    if (locked.current || !batch.length) return false
    locked.current = true
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
        const current = await getNotebook(neon.userId)
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
        applySnapshot(snapshot)
        if (offlineAccess()?.userId === neonUserId)
          await cacheOfflineRoster(neonUserId, snapshot).catch(() => {})
        localStorage.removeItem(key)
        return true
      })
      if (!saved) return false
      setPending([])
      setMessage(`${batch.length} record${batch.length === 1 ? '' : 's'} saved.`)
      return true
    } catch (cause) {
      setError(
        `${errorText(cause)}${writeStarted ? ' The save may already have reached the server. Retry the same pending batch to avoid counting it twice.' : ''}`,
      )
      return false
    } finally {
      locked.current = false
      setBusy(false)
    }
  }

  async function save(payloads: Payload[]): Promise<boolean> {
    if (storageBlocked || pending.length || !loaded || !payloads.length) return false
    if (offline) {
      try {
        if (hasForeignPending(key) || localStorage.getItem(key))
          throw new Error('Resolve the pending online batch before saving offline.')
        const snapshot = await getOfflineSnapshot(neonUserId)
        if (!snapshot || offlineAccess()?.userId !== neonUserId || !neon.teacher.canWrite)
          return false
        const marks: z.infer<typeof offlineAttendanceRecordSchema>[] = []
        const notes: z.infer<typeof offlineStudentOperationSchema>[] = []
        for (const payload of payloads) {
          if (payload.kind === 'attendance') {
            const enrollment = snapshot.courses
              .filter((course) => course.open)
              .flatMap((course) => course.enrollments.filter((entry) => entry.active))
              .find(
                (entry) =>
                  entry.id === payload.enrollmentId && entry.studentId === payload.studentId,
              )
            if (!enrollment) throw new Error('This student is not enrolled in an open course.')
            const current = snapshot.attendance.find(
              (entry) =>
                entry.studentId === payload.studentId &&
                entry.enrollmentId === payload.enrollmentId &&
                entry.date === payload.date,
            )
            marks.push(
              offlineAttendanceRecordSchema.parse({
                event: makeEvent(neon.teacher.name, payload),
                expectedEventId: current?.eventId ?? null,
              }),
            )
          } else if (
            payload.kind === 'note' &&
            snapshot.students.some((student) => student.id === payload.studentId)
          ) {
            notes.push(
              offlineStudentOperationSchema.parse({
                kind: 'note',
                event: makeEvent(neon.teacher.name, payload),
              }),
            )
          } else throw new Error('This action is unavailable offline.')
        }
        await withPendingLocks(key, async () => {
          if (offlineAccess()?.userId !== neonUserId)
            throw new Error('Offline access has expired or been withdrawn.')
          if (hasForeignPending(key) || localStorage.getItem(key))
            throw new Error('Resolve the pending online batch before saving offline.')
          await queueNotebookOperations(neonUserId, marks, notes)
        })
        await loadOffline()
        return true
      } catch (cause) {
        setError(errorText(cause))
        return false
      }
    }
    if (offlinePendingCount) {
      setError('Review pending offline changes before saving online.')
      return false
    }
    try {
      return await send(payloads.map((payload) => makeEvent(neon.teacher.name, payload)))
    } catch (cause) {
      setError(errorText(cause))
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

  const scopedEvents = useMemo(() => assignedEvents(events, roster), [events, roster])
  return {
    events: scopedEvents,
    roster,
    courses,
    enrollments,
    snapshotTeacher,
    pending,
    storageBlocked,
    loaded,
    busy,
    error,
    message,
    invalid,
    offline,
    offlineExpired,
    offlinePrepared,
    offlinePendingCount,
    saveOfflinePhone: async (
      studentId: string,
      expectedWhatsapp: string | null,
      whatsapp: string | null,
    ) => {
      if (
        !offline ||
        storageBlocked ||
        pending.length ||
        offlineAccess()?.userId !== neonUserId ||
        !roster.some((student) => student.studentId === studentId)
      )
        return false
      try {
        if (hasForeignPending(key) || localStorage.getItem(key))
          throw new Error('Resolve the pending online batch before saving offline.')
        await withPendingLocks(key, async () => {
          if (offlineAccess()?.userId !== neonUserId)
            throw new Error('Offline access has expired or been withdrawn.')
          if (hasForeignPending(key) || localStorage.getItem(key))
            throw new Error('Resolve the pending online batch before saving offline.')
          await queueStudentOperation(neonUserId, {
            kind: 'contact',
            id: crypto.randomUUID(),
            studentId,
            clientTime: new Date().toISOString(),
            expectedWhatsapp,
            whatsapp,
          })
        })
        await loadOffline()
        return true
      } catch (cause) {
        setError(errorText(cause))
        return false
      }
    },
    refresh,
    save,
    retry: () => (storageBlocked ? Promise.resolve(false) : send(pending, true)),
    discard,
  }
}
function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : 'The request failed.'
}
