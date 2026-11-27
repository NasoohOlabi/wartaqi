import { useEffect, useState } from 'react'
import { z } from 'zod'
import {
  applyGradeChanges,
  gradeChanges,
  gradeOperationSchema,
  gradeOutcomeSchema,
  gradeReviewSchema,
  gradePath,
  type GradeValues,
} from '../shared/gradeSync'
import { NeonRequestError, request } from './neon'
import { localGrant } from './offlineAccess'

const changedEvent = 'wartaq-grade-queue-change'
export const gradeAcceptedEvent = 'wartaq-grade-accepted'
const pendingSchema = z.object({
  operation: gradeOperationSchema,
  label: z.string(),
  outcome: gradeReviewSchema.nullable(),
})
export type PendingGrade = z.infer<typeof pendingSchema>
type QueuedGrade = PendingGrade & { generation: string }

export function gradeScope(userId: string) {
  const grant = localGrant(userId)
  return grant?.account.role === 'administrator' && grant.account.canWrite ? grant : null
}

type Scope = NonNullable<ReturnType<typeof gradeScope>>

function inScope(scope: Scope) {
  return gradeScope(scope.userId)?.generation === scope.generation
}

function gradeKey(scope: Scope, enrollmentId: string, generation = scope.generation) {
  return [scope.userId, generation, enrollmentId]
}

async function accessQueue<T>(
  mode: IDBTransactionMode,
  access: (store: IDBObjectStore) => IDBRequest<T>,
): Promise<T> {
  return new Promise((resolve, reject) => {
    const opening = indexedDB.open('wartaq-grade-sync', 1)
    opening.onupgradeneeded = () => opening.result.createObjectStore('grades')
    opening.onblocked = () => reject(new Error('Grade storage is blocked'))
    opening.onerror = () => reject(opening.error)
    opening.onsuccess = () => {
      const db = opening.result
      try {
        const tx = db.transaction('grades', mode)
        tx.onabort = () => {
          db.close()
          reject(tx.error)
        }
        const job = access(tx.objectStore('grades'))
        tx.oncomplete = () => {
          db.close()
          resolve(job.result)
        }
      } catch (error) {
        db.close()
        reject(error)
      }
    }
  })
}

async function pendingGrades(scope: Scope) {
  if (!inScope(scope)) return []
  const range = IDBKeyRange.bound(
    [scope.userId, scope.generation],
    [scope.userId, scope.generation, []],
  )
  const values: unknown = await accessQueue('readonly', (store) => store.getAll(range))
  return inScope(scope) ? z.array(pendingSchema).parse(values) : []
}

async function recoveredGrades(scope: Scope): Promise<QueuedGrade[]> {
  if (!inScope(scope)) return []
  const items: QueuedGrade[] = []
  const range = IDBKeyRange.bound([scope.userId], [scope.userId, []])
  await accessQueue('readonly', (store) => {
    const request = store.openCursor(range)
    request.onsuccess = () => {
      const cursor = request.result
      if (!cursor) return
      const key = z.tuple([z.string(), z.uuid(), z.uuid()]).safeParse(cursor.key)
      const grade = pendingSchema.safeParse(cursor.value)
      if (
        key.success &&
        grade.success &&
        key.data[1] !== scope.generation &&
        grade.data.operation.grant.id === scope.account.id
      )
        items.push({ ...grade.data, generation: key.data[1] })
      cursor.continue()
    }
    return request
  })
  return inScope(scope) ? items : []
}

function notifyQueueChanged() {
  window.dispatchEvent(new Event(changedEvent))
  try {
    localStorage.setItem(changedEvent, crypto.randomUUID())
  } catch {
    // Same-tab notification remains available after the write commits.
  }
}

export function subscribeGradeQueue(refresh: () => void) {
  function onStorage(event: StorageEvent) {
    if (event.key === changedEvent) refresh()
  }
  window.addEventListener(changedEvent, refresh)
  window.addEventListener('storage', onStorage)
  return () => {
    window.removeEventListener(changedEvent, refresh)
    window.removeEventListener('storage', onStorage)
  }
}

export function useGradeQueue(userId: string, kind: 'current' | 'recover' | 'hidden' = 'current') {
  const generation = kind === 'hidden' ? undefined : gradeScope(userId)?.generation
  const [snapshot, setSnapshot] = useState<{ generation: string; items: QueuedGrade[] }>()
  useEffect(() => {
    const scope = gradeScope(userId)
    if (!scope || scope.generation !== generation) return
    let active = true
    let version = 0
    function refresh() {
      if (!scope) return
      const requested = ++version
      const load =
        kind === 'recover'
          ? recoveredGrades(scope)
          : pendingGrades(scope).then((items) =>
              items.map((item) => ({ ...item, generation: scope.generation })),
            )
      void load
        .then((items) => {
          if (active && requested === version && inScope(scope))
            setSnapshot({ generation: scope.generation, items })
        })
        .catch(() => {})
    }
    refresh()
    const unsubscribe = subscribeGradeQueue(refresh)
    return () => {
      active = false
      unsubscribe()
    }
  }, [userId, generation, kind])
  return snapshot && snapshot.generation === generation ? snapshot.items : []
}

export async function queueGradeEdit({
  scope,
  id,
  label,
  previous,
  next,
}: {
  scope: Scope
  id: string
  label: string
  previous: GradeValues
  next: GradeValues
}) {
  if (!navigator.locks) throw new Error('Browser does not support safe grade sync')
  const fields = gradeChanges(previous, next)
  if (!Object.keys(fields).length) return
  const item = pendingSchema.parse({
    operation: {
      kind: 'grade-edit',
      requestId: crypto.randomUUID(),
      editTime: new Date().toISOString(),
      id,
      grant: { id: scope.account.id, expiresAt: scope.account.expiresAt },
      fields,
    },
    label,
    outcome: null,
  })
  await accessQueue('readwrite', (store) => {
    if (!inScope(scope)) throw new Error('Grade access changed')
    return store.add(item, gradeKey(scope, id))
  })
  notifyQueueChanged()
}

function gradeLock(scope: Scope) {
  return JSON.stringify(['wartaq-grade-sync', scope.userId, scope.account.id])
}

export async function replayGrades(scope: Scope, retryBlocked = false) {
  if (!inScope(scope) || !navigator.onLine || !navigator.locks) return
  await navigator.locks.request(gradeLock(scope), async () => {
    for (const item of await pendingGrades(scope)) {
      if (!inScope(scope) || !navigator.onLine) return
      if (item.outcome && !(retryBlocked && item.outcome.status === 'blocked')) continue
      if (!(await deliverGrade(scope, item, gradeKey(scope, item.operation.id)))) return
    }
  })
}

async function deliverGrade(scope: Scope, item: PendingGrade, key: string[], recover = false) {
  const body = recover
    ? {
        kind: 'recover-grade-edit',
        operation: item.operation,
        grant: { id: scope.account.id, expiresAt: scope.account.expiresAt },
      }
    : item.operation
  let outcome: z.infer<typeof gradeOutcomeSchema>
  try {
    outcome = gradeOutcomeSchema.parse(await request(gradePath, scope.userId, body))
  } catch (error) {
    if (!(error instanceof NeonRequestError) || ![400, 409, 413, 422].includes(error.status))
      return false
    outcome = { status: 'blocked' }
  }
  if (!inScope(scope)) return false
  if (outcome.status === 'accepted') {
    await accessQueue('readwrite', (store) => store.delete(key))
    if (inScope(scope))
      window.dispatchEvent(new CustomEvent(gradeAcceptedEvent, { detail: scope.generation }))
  } else {
    if (outcome.status === 'blocked' && item.outcome?.status === 'blocked') return true
    await accessQueue('readwrite', (store) => store.put({ ...item, outcome }, key))
  }
  notifyQueueChanged()
  return true
}

export async function retryRecoveredGrade(scope: Scope, item: QueuedGrade) {
  if (!inScope(scope) || !navigator.onLine || !navigator.locks) return
  await navigator.locks.request(gradeLock(scope), async () => {
    if (!inScope(scope)) return
    const key = gradeKey(scope, item.operation.id, item.generation)
    const current = pendingSchema.safeParse(
      await accessQueue('readonly', (store) => store.get(key)),
    )
    if (
      !current.success ||
      current.data.operation.requestId !== item.operation.requestId ||
      current.data.operation.grant.id !== scope.account.id ||
      current.data.outcome?.status === 'conflict' ||
      !inScope(scope)
    )
      return
    if (!(await deliverGrade(scope, current.data, key, true)))
      throw new Error('Grade recovery unavailable')
  })
}

export async function resolveGradeEdit(scope: Scope, item: QueuedGrade, keepMine: boolean) {
  if (!inScope(scope) || !navigator.locks) return
  await navigator.locks.request(gradeLock(scope), async () => {
    if (!inScope(scope)) return
    const key = gradeKey(scope, item.operation.id, item.generation)
    const current = pendingSchema.safeParse(
      await accessQueue('readonly', (store) => store.get(key)),
    )
    if (
      !current.success ||
      current.data.operation.requestId !== item.operation.requestId ||
      current.data.operation.grant.id !== scope.account.id ||
      !inScope(scope)
    )
      return
    const outcome = current.data.outcome
    if (keepMine && outcome?.status === 'conflict') {
      const desired = applyGradeChanges(outcome.current, item.operation.fields)
      const fields = gradeChanges(outcome.current, desired)
      const operation = gradeOperationSchema.parse({
        ...item.operation,
        requestId: crypto.randomUUID(),
        editTime: new Date().toISOString(),
        fields,
      })
      const next: PendingGrade = { ...item, operation, outcome: null }
      await accessQueue('readwrite', (store) => store.put(next, key))
    } else if (!keepMine) {
      await accessQueue('readwrite', (store) => store.delete(key))
    } else return
    notifyQueueChanged()
  })
}
