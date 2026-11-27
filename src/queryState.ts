import { useEffect, useState, useSyncExternalStore } from 'react'
import { z } from 'zod'

const tabSchema = z.enum(['dashboard', 'attendance', 'roster', 'summary'])
const settingsSectionSchema = z.enum(['accounts', 'students']).catch('accounts')
export type Tab = z.infer<typeof tabSchema>
export function today() {
  const date = new Date()
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
}
function subscribe(callback: () => void) {
  window.addEventListener('popstate', callback)
  return () => window.removeEventListener('popstate', callback)
}
export function useQueryState() {
  const search = useSyncExternalStore(subscribe, () => window.location.search)
  const query = sanitizedSearch(search)
  const explicitTab = tabSchema.safeParse(query.get('tab')).data
  const view =
    z
      .enum([
        'invitation-message',
        'settings',
        'administration',
        'add-student',
        'edit-student',
        'notebook',
        'courses',
        'student',
        'profile',
        'history',
        'diagnostics',
        'telegram',
        'changes',
      ])
      .safeParse(query.get('view')).data ?? 'notebook'
  return {
    page: z.enum(['home', 'app']).safeParse(query.get('page')).data ?? 'home',
    view: view === 'notebook' && query.get('tab') === 'history' ? 'history' : view,
    settingsSection: settingsSectionSchema.parse(query.get('settingsSection')),
    tab: explicitTab ?? 'dashboard',
    explicitTab,
    date: z.iso.date().safeParse(query.get('date')).data ?? today(),
    from: z.iso.date().safeParse(query.get('from')).data ?? today().slice(0, 7) + '-01',
    to: z.iso.date().safeParse(query.get('to')).data ?? today(),
    q: query.get('q') ?? '',
    group: query.get('group') ?? '',
    student: query.get('student') ?? '',
    inviteMessage: z.uuid().safeParse(query.get('inviteMessage')).data ?? '',
    account: z.string().min(1).max(256).safeParse(query.get('account')).data ?? '',
    course: z.uuid().safeParse(query.get('course')).data ?? '',
    courseScope:
      z.union([z.uuid(), z.literal('none'), z.literal('all')]).safeParse(query.get('course'))
        .data ?? '',
    courseView:
      z.enum(['create', 'edit', 'students', 'grades', 'history']).safeParse(query.get('courseView'))
        .data ?? '',
    studentMode:
      z.enum(['all', 'groups', 'books']).safeParse(query.get('studentMode')).data ?? 'all',
    all: query.get('all') === '1',
    missingScores: query.get('missing') === '1',
    unmarked: query.get('unmarked') === '1',
  }
}
const allowedKeys = new Set([
  'page',
  'inviteMessage',
  'login',
  'tab',
  'view',
  'settingsSection',
  'date',
  'from',
  'to',
  'q',
  'group',
  'student',
  'account',
  'course',
  'courseView',
  'studentMode',
  'all',
  'missing',
  'unmarked',
  'tables',
])
export function sanitizedSearch(search: string) {
  const query = new URLSearchParams(search)
  for (const key of [...query.keys()]) if (!allowedKeys.has(key)) query.delete(key)
  if (!['username', 'phone'].includes(query.get('login') ?? '')) query.delete('login')
  if (query.get('tab') === 'notes') {
    query.set('tab', 'attendance')
    for (const key of ['from', 'to', 'student', 'q', 'group']) query.delete(key)
  }
  const tables = query.get('tables')
  if (tables && tables.length <= maxTableStateLength && isLegacyTableState(tables))
    query.set('tables', encodeTableState(tables))
  return query
}
function publicHash(hash: string) {
  const value = hash.startsWith('#') ? hash.slice(1) : hash
  if (!value || new URLSearchParams(value).has('access')) return ''
  return `#${value}`
}

export function viewLinkFrom(url: URL) {
  const search = sanitizedSearch(url.search).toString()
  return `${url.origin}${url.pathname}${search ? `?${search}` : ''}${publicHash(url.hash)}`
}

export function currentViewLink() {
  return viewLinkFrom(new URL(window.location.href))
}

export function queryHref(values: Record<string, string>) {
  const query = sanitizedSearch(window.location.search)
  for (const [key, value] of Object.entries(values)) {
    if (!allowedKeys.has(key)) continue
    if (value) query.set(key, value)
    else query.delete(key)
  }
  const search = query.toString()
  return window.location.pathname + (search ? `?${search}` : '')
}

function writeQuery(values: Record<string, string>, push: boolean, child = false) {
  const href = queryHref(values)
  const current = window.location.pathname + window.location.search + window.location.hash
  if (href === current) return
  const state = push ? (child ? { workspaceChild: true } : null) : window.history.state
  window.history[push ? 'pushState' : 'replaceState'](state, '', href)
  window.dispatchEvent(new PopStateEvent('popstate'))
  if (push) window.scrollTo(0, 0)
}

export function setQuery(values: Record<string, string>, push = false) {
  writeQuery(values, push)
}

export function openChildView(values: Record<string, string>) {
  writeQuery(values, true, true)
}

export function returnToParent(fallback: Record<string, string>) {
  if (window.history.state?.workspaceChild === true) window.history.back()
  else setQuery(fallback)
}

export function courseListBack(ar: boolean, { clearStudentFilters = false } = {}) {
  return {
    label: ar ? 'كل الدورات' : 'All courses',
    onClick: () =>
      setQuery(
        {
          view: 'courses',
          courseView: '',
          course: '',
          q: '',
          ...(clearStudentFilters ? { group: '', student: '' } : {}),
        },
        true,
      ),
  }
}

export function navigatePage(page: 'home' | 'app') {
  window.history.pushState(null, '', window.location.pathname + (page === 'app' ? '?page=app' : ''))
  window.dispatchEvent(new PopStateEvent('popstate'))
  window.scrollTo(0, 0)
}

const coursePreferenceSchema = z.union([z.literal(''), z.uuid()])
const coursePreferenceKey = (accountId: string) => `halaqa-course:${accountId}`

function saveCoursePreference(accountId: string, course: string) {
  try {
    localStorage.setItem(coursePreferenceKey(accountId), JSON.stringify(course))
  } catch {
    // The URL remains usable when preference storage is unavailable.
  }
}

export function useCoursePreference(
  accountId: string | undefined,
  loaded: boolean,
  courses: { id: string }[],
) {
  const [ready, setReady] = useState(!accountId)
  const [defaultCourse, setDefaultCourse] = useState('')
  useEffect(() => {
    if (!accountId || !loaded || ready) return
    let selection = ''
    try {
      const stored = localStorage.getItem(coursePreferenceKey(accountId))
      if (stored !== null)
        selection = coursePreferenceSchema.safeParse(JSON.parse(stored)).data ?? ''
    } catch {
      // Ignore an unavailable or malformed preference.
    }
    if (selection && !courses.some((course) => course.id === selection)) selection = ''
    const query = new URLSearchParams(window.location.search)
    const notebook = !query.has('view') || query.get('view') === 'notebook'
    const explicit = query.get('course')
    if (notebook && explicit === null && selection) setQuery({ course: selection })
    else if (
      explicit &&
      explicit !== 'all' &&
      explicit !== 'none' &&
      (!z.uuid().safeParse(explicit).success || !courses.some((course) => course.id === explicit))
    ) {
      setQuery({ course: 'none' })
    }
    // oxlint-disable-next-line react/set-state-in-effect
    setDefaultCourse(selection)
    setReady(true)
  }, [accountId, loaded, courses, ready])
  function chooseDefault(course: string) {
    if (accountId) saveCoursePreference(accountId, course)
    setDefaultCourse(course)
    const scope = course || 'none'
    if (new URLSearchParams(window.location.search).get('course') !== scope)
      setQuery({ course: scope, student: '' }, true)
  }
  return { ready, defaultCourse, chooseDefault }
}

export function navigateNotebook(
  tab: Tab,
  defaultCourse: string | undefined,
  values: Record<string, string> = {},
) {
  setQuery(
    {
      ...values,
      view: 'notebook',
      tab,
      course: defaultCourse === undefined ? '' : defaultCourse || 'none',
    },
    true,
  )
}

const tableStateSchema = z.object({
  sorting: z.array(z.object({ id: z.string().max(100), desc: z.boolean() })).max(30),
  filters: z.array(z.object({ id: z.string().max(100), value: z.string().max(200) })).max(30),
})
type TableState = z.infer<typeof tableStateSchema>
const emptyTableState: TableState = { sorting: [], filters: [] }
const maxTableStateLength = 10000

function isLegacyTableState(value: string) {
  return value.trimStart().startsWith('{')
}

function encodeTableState(json: string) {
  // Some share targets decode URL escapes before detecting links in pasted text.
  const bytes = Array.from(new TextEncoder().encode(json), (byte) => String.fromCharCode(byte))
  return btoa(bytes.join('')).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '')
}

function readTableStates(search: string) {
  const raw = new URLSearchParams(search).get('tables')
  // UTF-8 can use three bytes per UTF-16 code unit, then Base64 uses four characters per three bytes.
  if (!raw || raw.length > maxTableStateLength * 4) return {}
  try {
    const json = isLegacyTableState(raw)
      ? raw
      : new TextDecoder('utf-8', { fatal: true }).decode(
          Uint8Array.from(atob(raw.replaceAll('-', '+').replaceAll('_', '/')), (byte) =>
            byte.charCodeAt(0),
          ),
        )
    if (json.length > maxTableStateLength) return {}
    return z.record(z.string().max(100), tableStateSchema).safeParse(JSON.parse(json)).data ?? {}
  } catch {
    return {}
  }
}
export function useTableQuery(id: string) {
  const search = useSyncExternalStore(subscribe, () => window.location.search)
  return readTableStates(search)[id] ?? emptyTableState
}
export function setTableQuery(id: string, state: TableState) {
  const states = readTableStates(window.location.search)
  if (!state.sorting.length && !state.filters.length) delete states[id]
  else states[id] = state
  setQuery({ tables: Object.keys(states).length ? encodeTableState(JSON.stringify(states)) : '' })
}
