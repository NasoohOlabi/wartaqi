import { createAuthClient } from 'better-auth/react'
import { z } from 'zod'
import { accountFetch, newEditVersion } from './accountRequest'
import { grantChangeEvent, grantStorageKey, localGrant } from './offlineAccess'
import { neonAccountSchema, neonBatchSchema, neonSnapshotSchema } from '../shared/neon'
import type { LedgerEvent } from '../shared/ledger'
import { notebookSummarySchema } from './notebookSummary'

export const apiUrl = window.location.origin
export const authClient = createAuthClient({ sessionOptions: { refetchInterval: 60 * 60 } })

export const sessionChangeEvent = 'halaqa-session-change'
export const sessionChangeSource = crypto.randomUUID()

export function announceSessionChange() {
  try {
    localStorage.setItem(sessionChangeEvent, crypto.randomUUID())
  } catch {
    /* Storage can be disabled. */
  }
  try {
    const channel = new BroadcastChannel(sessionChangeEvent)
    channel.postMessage(sessionChangeSource)
    channel.close()
  } catch {
    /* BroadcastChannel can be unavailable. */
  }
}

export function resetWorkspace() {
  announceSessionChange()
  window.location.replace(window.location.pathname + '?page=app')
}

export class NeonRequestError extends Error {
  constructor(public status: number) {
    super(
      status === 403
        ? 'Your account is awaiting approval or no longer has access. Contact your manager.'
        : status === 401
          ? 'Your session has expired. Please sign in again.'
          : 'The request failed. Please try again.',
    )
  }
}

export class AccessDeniedEvent extends Event {
  static readonly eventName = 'neon-access-denied'

  constructor(
    readonly userId: string,
    readonly status: 401 | 403,
  ) {
    super(AccessDeniedEvent.eventName)
  }
}

const privateReadPaths = new Set(['/api/notebook', '/api/admin', '/api/student/attendance'])
const notebookViewSchema = neonSnapshotSchema.extend({
  focus: z
    .discriminatedUnion('kind', [
      z.object({ kind: z.literal('session'), recordedEnrollmentIds: z.array(z.uuid()) }),
      z.object({ kind: z.literal('summary'), summary: notebookSummarySchema }),
      z.object({ kind: z.literal('register'), summary: notebookSummarySchema }),
      z.object({ kind: z.literal('student') }),
      z.object({ kind: z.literal('catalog') }),
      z.object({
        kind: z.literal('history'),
        nextBefore: z
          .string()
          .regex(/^[1-9]\d{0,18}$/)
          .nullable(),
        reversedIds: z.array(z.uuid()),
      }),
    ])
    .optional(),
})
export type NotebookView = z.infer<typeof notebookViewSchema>
export type NotebookFocus =
  | { kind: 'session'; date: string; course: string }
  | { kind: 'summary'; course: string }
  | { kind: 'student'; student: string }
  | { kind: 'register' }
  | { kind: 'catalog' }
  | { kind: 'history'; course: string; student: string; before: string }

export function savesNetworkData() {
  return Boolean(
    (navigator as Navigator & { connection?: { saveData?: boolean } }).connection?.saveData,
  )
}
let privateReadScope = ''
let privateReadSequence = 0
const privateReads = new Map<string, { path: string; validator: string; value: unknown }>()
const privateReadVersions = new Map<string, number>()
function clearPrivateReads() {
  privateReads.clear()
  privateReadVersions.clear()
  privateReadScope = ''
  privateReadSequence++
}
window.addEventListener(grantChangeEvent, clearPrivateReads)
window.addEventListener('storage', (event) => {
  if (event.key === grantStorageKey || event.key === null) clearPrivateReads()
})

export async function request(
  path: string,
  userId: string,
  body?: unknown,
  reportDenial = true,
  edit = body !== undefined ? newEditVersion() : undefined,
  signal?: AbortSignal,
): Promise<unknown> {
  const generation = localGrant(userId)?.generation
  const scope = generation ? JSON.stringify([userId, generation]) : ''
  if (scope !== privateReadScope) {
    clearPrivateReads()
    privateReadScope = scope
  }
  const endpoint = path.split('?', 1)[0]
  const cacheable = body === undefined && !!scope && privateReadPaths.has(endpoint)
  const entry = cacheable ? privateReads.get(endpoint) : undefined
  const cached = entry?.path === path ? entry : undefined
  const epoch = privateReadSequence
  const version = cacheable ? (privateReadVersions.get(endpoint) ?? 0) + 1 : 0
  if (cacheable) privateReadVersions.set(endpoint, version)
  const deny = (status: 401 | 403) => {
    if (privateReadScope === scope) {
      clearPrivateReads()
    }
    if (reportDenial && localGrant(userId)?.generation === generation)
      window.dispatchEvent(new AccessDeniedEvent(userId, status))
  }
  let response = await accountFetch(path, userId, body, edit, cached?.validator, signal)
  if (response.status === 304) {
    if (
      cached &&
      privateReadScope === scope &&
      localGrant(userId)?.generation === generation &&
      privateReads.get(endpoint) === cached
    )
      return cached.value
    if (privateReadScope !== scope || localGrant(userId)?.generation !== generation)
      throw new NeonRequestError(409)
    response = await accountFetch(path, userId, body, edit, undefined, signal)
  }
  if (!response.ok) {
    if (response.status === 401 || response.status === 403) deny(response.status)
    throw new NeonRequestError(response.status)
  }
  const value: unknown = await response.json()
  const validator = response.headers.get('ETag')
  if (
    cacheable &&
    validator &&
    privateReadScope === scope &&
    localGrant(userId)?.generation === generation &&
    epoch === privateReadSequence &&
    version === privateReadVersions.get(endpoint)
  )
    privateReads.set(endpoint, { path, validator, value })
  return value
}
export type InvitationProfile = z.infer<
  typeof import('../shared/invitations').invitationProfileSchema
>

export async function getAccount(userId: string) {
  return z
    .object({ account: neonAccountSchema })
    .parse(await request('/api/me', userId, undefined, false)).account
}

export class InvalidNotebookSnapshotError extends Error {
  constructor() {
    super('The request failed. Please try again.')
  }
}

export async function getNotebook(userId: string) {
  const snapshot = await getNotebookView(userId)
  if (snapshot.focus) throw new InvalidNotebookSnapshotError()
  return snapshot
}

export async function getNotebookView(userId: string, focus?: NotebookFocus, signal?: AbortSignal) {
  const query = new URLSearchParams()
  if (focus) {
    query.set('focusView', focus.kind)
    if (focus.kind === 'student') query.set('focusStudent', focus.student)
    else if (focus.kind === 'history') {
      if (focus.course) query.set('focusCourse', focus.course)
      if (focus.student) query.set('focusStudent', focus.student)
      if (focus.before) query.set('focusBefore', focus.before)
    } else if (focus.kind === 'session' || focus.kind === 'summary') {
      if (focus.kind === 'session') query.set('focusDate', focus.date)
      if (focus.course) query.set('focusCourse', focus.course)
    }
  }
  try {
    const path = focus ? `/api/notebook?${query}` : '/api/notebook'
    const snapshot = notebookViewSchema.parse(
      await request(path, userId, undefined, true, undefined, signal),
    )
    if (snapshot.focus && snapshot.focus.kind !== focus?.kind)
      throw new InvalidNotebookSnapshotError()
    return snapshot
  } catch (cause) {
    if (cause instanceof NeonRequestError) throw cause
    if (cause instanceof z.ZodError || cause instanceof SyntaxError)
      throw new InvalidNotebookSnapshotError()
    throw new Error('The request failed. Please try again.')
  }
}
export async function postEvents(userId: string, events: LedgerEvent[]) {
  try {
    return neonSnapshotSchema.parse(
      await request('/api/events', userId, neonBatchSchema.parse({ events })),
    )
  } catch (cause) {
    if (cause instanceof NeonRequestError) throw cause
    throw new Error('The request failed. Please try again.')
  }
}

export async function postCorrections(userId: string, events: LedgerEvent[]) {
  const { saved } = z
    .object({ saved: z.int() })
    .parse(await request('/api/events/correct', userId, neonBatchSchema.parse({ events })))
  if (saved !== events.length)
    throw new Error('The correction response did not match the submitted batch.')
}
