import './course-sections.css'
import { WorkspacePageHeader } from './WorkspacePageHeader'
import { RowActions } from './RowActions'
import { formatLevel } from './levelFormat'
import { TableControls, TableHeader, TableToolbar } from './DataTable'
import { useDataTable } from './useDataTable'
import { useEffect, useRef, useState } from 'react'
import { useAdminBoardReplica } from './useRoleReplica'
import {
  Check,
  ClipboardList,
  Download,
  History,
  Lock,
  LockOpen,
  Pencil,
  Plus,
  Save,
  Star,
  UsersRound,
  UserRoundCog,
} from 'lucide-react'
import {
  accountProfileLabel,
  neonAdminBoardSchema,
  type Course,
  type Enrollment,
} from '../shared/neon'
import { projectEvents, type LedgerEvent } from '../shared/ledger'
import { courseAttendance, missingCourseScores, savedGradeResult } from '../shared/courseGrading'
import { applyGradeChanges, type GradeValues } from '../shared/gradeSync'
import { gradeAcceptedEvent, gradeScope, queueGradeEdit, useGradeQueue } from './gradeSync'
import type { CachedAccess } from './offlineAccess'
import { CourseExportDialog } from './CourseExportDialog'
import { request } from './neon'
import { useLanguage } from './useLanguage'
import { courseListBack, openChildView, setQuery, useQueryState } from './queryState'
import { rankStudents } from './studentSearch'

type Board = ReturnType<typeof neonAdminBoardSchema.parse>

type Draft = { id?: string; teacherId?: string }

function CourseGroups({
  ar,
  board,
  enrollments,
  disabled,
  move,
}: {
  ar: boolean
  board: Board
  enrollments: Enrollment[]
  disabled: boolean
  move: (enrollmentId: string, teacherId: Enrollment['teacherId']) => void
}) {
  const unassigned = ar ? 'غير معيّن' : 'Unassigned'
  const moveLabel = ar ? 'نقل إلى مجموعة' : 'Move to group'
  const groups = new Map<
    Enrollment['teacherId'],
    { name: string; members: { name: string; enrollmentId: string }[] }
  >()
  for (const teacher of board.teachers)
    groups.set(teacher.teacherId, { name: teacher.name, members: [] })
  groups.set(null, { name: unassigned, members: [] })
  const activeByStudent = new Map(
    enrollments.filter((entry) => entry.active).map((entry) => [entry.studentId, entry]),
  )
  for (const student of board.students) {
    const entry = activeByStudent.get(student.studentId)
    if (!entry) continue
    const group = groups.get(entry.teacherId) ?? {
      name: entry.teacherName ?? (ar ? 'معلم غير متاح' : 'Unavailable teacher'),
      members: [],
    }
    group.members.push({ name: student.name, enrollmentId: entry.id })
    groups.set(entry.teacherId, group)
  }
  const destinations = [
    ...board.teachers.map(({ teacherId, name }) => ({ teacherId, name })),
    { teacherId: null, name: unassigned },
  ]
  return [...groups].map(([teacherId, group]) => {
    const options = destinations.filter((destination) => destination.teacherId !== teacherId)
    return (
      <details
        key={teacherId ?? 'unassigned'}
        className="rounded-lg border border-[var(--dark-border,#cde1df)] px-3"
      >
        <summary className="min-h-11 cursor-pointer py-2.5 font-medium [overflow-wrap:anywhere]">
          <bdi>{group.name}</bdi> ({group.members.length})
        </summary>
        {group.members.length ? (
          <ul className="space-y-2 pb-3">
            {group.members.map(({ name, enrollmentId }) => (
              <li key={enrollmentId} className="flex flex-wrap items-center justify-between gap-2">
                <bdi className="min-w-0 [overflow-wrap:anywhere]">{name}</bdi>
                {options.length > 0 && (
                  <select
                    className="min-h-11 min-w-0 max-w-full"
                    aria-label={`${moveLabel}: ${name}`}
                    value=""
                    disabled={disabled}
                    onChange={(event) => {
                      const destination = event.target.value
                      move(enrollmentId, destination === 'unassigned' ? null : destination)
                    }}
                  >
                    <option value="" disabled>
                      {moveLabel}
                    </option>
                    {options.map((destination) => (
                      <option
                        key={destination.teacherId ?? 'unassigned'}
                        value={destination.teacherId ?? 'unassigned'}
                      >
                        {destination.name}
                      </option>
                    ))}
                  </select>
                )}
              </li>
            ))}
          </ul>
        ) : (
          <p className="muted pb-3">{ar ? 'لا يوجد طلاب' : 'No students'}</p>
        )}
      </details>
    )
  })
}

function CourseForm({
  course,
  ar,
  busy,
  submit,
}: {
  course?: Course
  ar: boolean
  busy: boolean
  submit: (id: string, name: string) => Promise<void>
}) {
  const [id] = useState(() => course?.id ?? crypto.randomUUID())
  return (
    <form
      className="flex flex-wrap items-end gap-3"
      onSubmit={(event) => {
        event.preventDefault()
        const name = new FormData(event.currentTarget).get('name')
        if (typeof name === 'string') void submit(id, name)
      }}
    >
      <label>
        {ar ? 'اسم الدورة' : 'Course name'}
        <input name="name" required maxLength={200} defaultValue={course?.name ?? ''} />
      </label>
      <button className="primary" disabled={busy}>
        {ar ? 'حفظ الدورة' : 'Save course'}
      </button>
    </form>
  )
}

function CourseSections({
  courseId,
  ar,
  current,
}: {
  courseId: string
  ar: boolean
  current: 'students' | 'groups' | 'grades' | 'history'
}) {
  const sections = [
    {
      id: 'students',
      label: ar ? 'الطلاب' : 'Students',
      accessible: ar ? 'إدارة طلاب الدورة' : 'Manage course students',
    },
    {
      id: 'groups',
      label: ar ? '\u0627\u0644\u0645\u062c\u0645\u0648\u0639\u0627\u062a' : 'Groups',
      accessible: ar
        ? '\u0625\u062f\u0627\u0631\u0629 \u0627\u0644\u0645\u062c\u0645\u0648\u0639\u0627\u062a'
        : 'Manage groups',
    },
    {
      id: 'grades',
      label: ar ? 'الدرجات' : 'Grades',
      accessible: ar ? 'درجات الدورة' : 'Course grades',
    },
    {
      id: 'history',
      label: ar ? 'السجل' : 'History',
      accessible: ar ? 'سجل الدورة' : 'Course history',
    },
  ] as const
  return (
    <nav aria-label={ar ? 'أقسام الدورة' : 'Course sections'} className="workspace-sections">
      {sections.map(({ id, label, accessible }) => (
        <a
          key={id}
          aria-label={accessible}
          onClick={(event) => {
            if (
              event.button !== 0 ||
              event.metaKey ||
              event.ctrlKey ||
              event.shiftKey ||
              event.altKey
            )
              return
            event.preventDefault()
            if (current === id) return
            setQuery(
              {
                view: 'courses',
                course: courseId,
                courseView: id,
                tab: '',
                q: '',
                group: '',
                student: '',
              },
              true,
            )
          }}
          aria-current={current === id ? 'page' : undefined}
          href={`?${new URLSearchParams({ page: 'app', view: 'courses', course: courseId, courseView: id })}`}
        >
          {label}
        </a>
      ))}
    </nav>
  )
}

export function CoursePageHeader({
  course,
  ar,
  current,
  actions,
}: {
  course: Pick<Course, 'id' | 'name'>
  ar: boolean
  current: 'students' | 'groups' | 'grades' | 'history'
  actions?: React.ReactNode
}) {
  const section = {
    students: ar
      ? '\u0625\u062f\u0627\u0631\u0629 \u0627\u0644\u0637\u0644\u0627\u0628'
      : 'Manage students',
    groups: ar
      ? '\u0625\u062f\u0627\u0631\u0629 \u0627\u0644\u0645\u062c\u0645\u0648\u0639\u0627\u062a'
      : 'Manage groups',
    grades: ar ? '\u0627\u0644\u062f\u0631\u062c\u0627\u062a' : 'Grades',
    history: ar ? '\u0627\u0644\u0633\u062c\u0644' : 'History',
  }[current]
  const back = courseListBack(ar)
  return (
    <WorkspacePageHeader
      parent={
        <button className="secondary workspace-page-back" onClick={back.onClick}>
          {back.label}
        </button>
      }
      title={
        <>
          <bdi>{course.name}</bdi>
          {' \u2014 '}
          {section}
        </>
      }
      actions={actions}
    >
      <CourseSections courseId={course.id} ar={ar} current={current} />
    </WorkspacePageHeader>
  )
}

export function Courses({
  userId,
  courses,
  enrollments,
  gradeEnrollments,
  events,
  refreshAfterWrite,
  defaultCourse,
  chooseDefault,
  cached,
}: {
  userId: string
  courses: Course[]
  enrollments: Enrollment[]
  gradeEnrollments: Enrollment[]
  events: LedgerEvent[]
  refreshAfterWrite: () => Promise<boolean>
  defaultCourse: string
  chooseDefault: (course: string) => void
  cached?: CachedAccess
}) {
  const { language } = useLanguage()
  const ar = language === 'ar'
  const addStudentLabel = ar ? 'إضافة طالب جديد إلى هذه الدورة' : 'Add new student to this course'
  const query = useQueryState()
  const course = courses.find((item) => item.id === query.course)
  const { data: board, error: boardError, refresh: refreshBoard } = useAdminBoardReplica()
  const offline = Boolean(cached)
  const offlineRef = useRef(offline)
  useEffect(() => {
    offlineRef.current = offline
  }, [offline])
  const [drafts, setDrafts] = useState<Record<string, Draft>>({})
  const [error, setError] = useState('')
  const [saved, setSaved] = useState(false)
  const [busy, setBusy] = useState(false)
  const [selected, setSelected] = useState<Record<string, string[]>>({})
  const [bulkTeacher, setBulkTeacher] = useState('unchanged')
  const [failedSaves, setFailedSaves] = useState<Record<string, () => void>>({})
  const [pendingMembership, setPendingMembership] = useState<Record<string, boolean>>({})
  const writeQueue = useRef<Promise<void>>(Promise.resolve())
  const readbackQueue = useRef<Promise<void>>(Promise.resolve())
  const pendingSaves = useRef(0)
  useEffect(() => {
    if (!saved) return
    const timer = window.setTimeout(() => setSaved(false), 4_000)
    return () => window.clearTimeout(timer)
  }, [saved])
  function submit(
    path: string,
    body: unknown,
    onSuccess?: () => void,
    saveKey?: string,
    options?: { refreshOnly?: boolean; membership?: { key: string; active: boolean } },
  ) {
    const { refreshOnly = false, membership } = options ?? {}
    if (offline) return Promise.resolve(false)
    if (membership)
      setPendingMembership((current) => ({ ...current, [membership.key]: membership.active }))
    pendingSaves.current += 1
    setBusy(true)
    const write = writeQueue.current.then(async () => {
      if (offlineRef.current) throw new Error('Connect to Wartaq to save course changes.')
      if (!refreshOnly) await request(path, userId, body)
    })
    writeQueue.current = write.catch(() => undefined)
    const operation = readbackQueue.current.then(async () => {
      setError('')
      setSaved(false)
      if (saveKey)
        setFailedSaves((current) => {
          const next = { ...current }
          delete next[saveKey]
          return next
        })
      let writeSucceeded = refreshOnly
      try {
        await write
        writeSucceeded = true
        if (path === '/api/admin/courses/status') {
          await refreshBoard()
        }
        if (!(await refreshAfterWrite()) && saveKey)
          throw new Error('Could not refresh student changes')
        onSuccess?.()
        setSaved(true)
        return true
      } catch (cause) {
        if (saveKey)
          setFailedSaves((current) => ({
            ...current,
            [saveKey]: () =>
              void submit(path, body, onSuccess, saveKey, {
                refreshOnly: writeSucceeded,
                membership,
              }),
          }))
        else setError(cause instanceof Error ? cause.message : 'Save failed')
        return false
      } finally {
        if (membership)
          setPendingMembership((current) => {
            const next = { ...current }
            delete next[membership.key]
            return next
          })
        pendingSaves.current -= 1
        if (!pendingSaves.current) setBusy(false)
      }
    })
    readbackQueue.current = operation.then(() => undefined)
    return operation
  }
  const go = (courseView: string, courseId = query.course) =>
    setQuery({ courseView, course: courseId, q: '', group: '', student: '' }, true)
  const back = courseListBack(ar, { clearStudentFilters: true })
  const courseEntries = enrollments.filter((item) => item.courseId === query.course)
  const matching = board ? rankStudents(board.students, query.q) : []
  const courseTable = useDataTable('courses', courses, [
    { id: 'name', label: ar ? 'الدورة' : 'Course', value: (item) => item.name },
    {
      id: 'status',
      choices: ar ? ['مفتوحة', 'مغلقة'] : ['Open', 'Closed'],
      label: ar ? 'الحالة' : 'Status',
      value: (item) => (item.open ? (ar ? 'مفتوحة' : 'Open') : ar ? 'مغلقة' : 'Closed'),
    },
  ])
  const membershipColumns: Parameters<typeof useDataTable<(typeof matching)[number]>>[2] = [
    { id: 'first', label: ar ? 'الاسم الأول' : 'First name', value: (s) => s.firstName },
    { id: 'last', label: ar ? 'اسم العائلة' : 'Last name', value: (s) => s.lastName },
    {
      id: 'course-level',
      label: ar ? 'مستوى الدورة' : 'Course level',
      value: (s) =>
        formatLevel(
          courseEntries.find((entry) => entry.studentId === s.studentId)?.level ?? s.group,
        ),
    },
    {
      id: 'teacher',
      label: ar ? 'المعلم' : 'Teacher',
      value: (s) => {
        const teacherId =
          drafts[`${course?.id}:${s.studentId}`]?.teacherId ??
          courseEntries.find((entry) => entry.studentId === s.studentId)?.teacherId
        return board?.teachers.find((teacher) => teacher.teacherId === teacherId)?.name ?? ''
      },
    },
    {
      id: 'enrolled',
      choices: ar ? ['نعم', 'لا'] : ['Yes', 'No'],
      label: ar ? 'مسجل' : 'Enrolled',
      value: (s) =>
        courseEntries.find((entry) => entry.studentId === s.studentId)?.active
          ? ar
            ? 'نعم'
            : 'Yes'
          : ar
            ? 'لا'
            : 'No',
    },
    {
      id: 'handbook',
      choices: ar ? ['نعم', 'لا'] : ['Yes', 'No'],
      label: ar ? 'استلم الكتاب' : 'Handbook received',
      value: (s) =>
        courseEntries.find((entry) => entry.studentId === s.studentId)?.handbookReceived
          ? ar
            ? 'نعم'
            : 'Yes'
          : ar
            ? 'لا'
            : 'No',
    },
  ]
  const membershipTable = useDataTable(
    'course-students',
    matching,
    membershipColumns.filter(
      (column) =>
        query.studentMode === 'all' ||
        (query.studentMode === 'groups'
          ? column.id !== 'handbook'
          : ['first', 'last', 'enrolled', 'handbook'].includes(column.id)),
    ),
  )
  const selectedIds = selected[course?.id ?? ''] ?? []
  const visibleIds = membershipTable.rows.flatMap((student) => {
    const entry = courseEntries.find((item) => item.studentId === student.studentId && item.active)
    return entry ? [entry.id] : []
  })
  const retries = Object.values(failedSaves)
  return (
    <section className="panel space-y-5">
      <div className="course-save-status" role="status">
        {saved && (
          <span className="flex items-center gap-2 rounded-lg bg-[var(--dark-active,#004d57)] px-4 py-2.5 text-sm font-medium text-white shadow-lg">
            <Check aria-hidden="true" size={18} />
            {ar ? 'تم الحفظ' : 'Saved'}
          </span>
        )}
      </div>
      {!query.courseView && (
        <TableToolbar model={courses.length ? courseTable : undefined}>
          <h1 className="title">{ar ? 'الدورات' : 'Courses'}</h1>
        </TableToolbar>
      )}
      {cached?.offline && (
        <p role="status">
          {query.courseView === 'grades'
            ? ar
              ? 'اتصل بوارتق لتغيير الحدود أو تعديل النتائج أو إغلاق الدورة.'
              : 'Connect to Wartaq to change thresholds, override results, or close the course.'
            : ar
              ? 'اتصل بوارتق لحفظ تغييرات الدورة.'
              : 'Connect to Wartaq to save course changes.'}
        </p>
      )}
      {(error || boardError) && <p role="alert">{error || boardError}</p>}
      {retries.length > 0 && (
        <div className="flex flex-wrap items-center gap-3">
          <p role="alert">
            {ar ? 'تعذر تأكيد حفظ تغييرات الطلاب.' : 'Could not confirm the student changes.'}
          </p>
          <button
            className="secondary"
            disabled={busy || offline}
            onClick={() => retries.forEach((retry) => retry())}
          >
            {ar ? 'إعادة المحاولة' : 'Retry'}
          </button>
        </div>
      )}
      {!query.courseView && (
        <>
          <button className="primary" onClick={() => go('create', '')}>
            {ar ? 'إنشاء دورة' : 'Create course'}
          </button>
          {!courses.length && <p>{ar ? 'لا توجد دورات بعد.' : 'No courses yet.'}</p>}
          {courses.length > 0 && (
            <div
              className="role-table-scroll course-list-scroll"
              role="region"
              aria-label={ar ? 'قائمة الدورات' : 'Course list'}
              tabIndex={0}
            >
              <table className="role-table course-list-table w-full table-fixed" role="table">
                <TableHeader model={courseTable} actions />
                <tbody role="rowgroup">
                  {courseTable.rows.map((item) => (
                    <tr key={item.id} role="row">
                      <td role="cell">
                        <a
                          className="course-name-link"
                          onClick={(event) => {
                            if (
                              event.button !== 0 ||
                              event.metaKey ||
                              event.ctrlKey ||
                              event.shiftKey ||
                              event.altKey
                            )
                              return
                            event.preventDefault()
                            go('students', item.id)
                          }}
                          href={`?${new URLSearchParams({ page: 'app', view: 'courses', course: item.id, courseView: 'students' })}`}
                        >
                          <bdi>{item.name}</bdi>
                        </a>
                        {item.id === defaultCourse && (
                          <span className="course-current">
                            {ar ? 'الدورة الحالية' : 'Current course'}
                          </span>
                        )}
                      </td>
                      <td role="cell">
                        {item.open ? (ar ? 'مفتوحة' : 'Open') : ar ? 'مغلقة' : 'Closed'}
                      </td>
                      <td role="cell">
                        <RowActions
                          name={item.name}
                          rowShortcut
                          ar={ar}
                          inlineIcons
                          actions={[
                            ...(item.id === defaultCourse
                              ? []
                              : [
                                  {
                                    label: ar ? 'تعيين كافتراضية' : 'Make default',
                                    icon: Star,
                                    disabled: !item.open,
                                    onClick: () => chooseDefault(item.id),
                                  },
                                ]),
                            {
                              label: ar ? 'إدارة الطلاب' : 'Manage students',
                              icon: UsersRound,
                              onClick: () => go('students', item.id),
                            },
                            {
                              label: ar
                                ? '\u0625\u062f\u0627\u0631\u0629 \u0627\u0644\u0645\u062c\u0645\u0648\u0639\u0627\u062a'
                                : 'Manage groups',
                              icon: UserRoundCog,
                              onClick: () => go('groups', item.id),
                            },
                            {
                              label: ar ? 'الدرجات' : 'Grades',
                              icon: ClipboardList,
                              onClick: () => go('grades', item.id),
                            },
                            {
                              label: ar ? 'السجل' : 'History',
                              icon: History,
                              onClick: () => go('history', item.id),
                            },
                            {
                              label: ar ? 'تعديل' : 'Edit',
                              icon: Pencil,
                              onClick: () => go('edit', item.id),
                            },
                            {
                              label: item.open
                                ? ar
                                  ? 'إغلاق الدورة'
                                  : 'Close course'
                                : ar
                                  ? 'إعادة فتح الدورة'
                                  : 'Reopen course',
                              icon: item.open ? Lock : LockOpen,
                              disabled: busy || offline,
                              onClick: () =>
                                void submit('/api/admin/courses/status', {
                                  id: item.id,
                                  open: !item.open,
                                }),
                            },
                          ]}
                        />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
      {(query.courseView === 'create' || (query.courseView === 'edit' && course)) && (
        <>
          <WorkspacePageHeader
            parent={
              <button className="secondary workspace-page-back" onClick={back.onClick}>
                {back.label}
              </button>
            }
            title={
              query.courseView === 'create'
                ? ar
                  ? '\u0625\u0646\u0634\u0627\u0621 \u062f\u0648\u0631\u0629'
                  : 'Create course'
                : ar
                  ? '\u062a\u0639\u062f\u064a\u0644 \u0627\u0644\u062f\u0648\u0631\u0629'
                  : 'Edit course'
            }
          />
          <CourseForm
            key={query.courseView === 'create' ? 'create' : course?.id}
            course={query.courseView === 'edit' ? course : undefined}
            ar={ar}
            busy={busy || offline}
            submit={async (id, name) => {
              if (await submit('/api/admin/courses', { id, name })) go('', '')
            }}
          />
        </>
      )}
      {query.courseView && query.courseView !== 'create' && !course && (
        <WorkspacePageHeader
          title={
            ar
              ? '\u0627\u0644\u062f\u0648\u0631\u0629 \u063a\u064a\u0631 \u0645\u062a\u0627\u062d\u0629'
              : 'Course unavailable'
          }
          parent={
            <button className="secondary workspace-page-back" onClick={back.onClick}>
              {back.label}
            </button>
          }
        />
      )}
      {course && query.courseView === 'groups' && (
        <>
          <CoursePageHeader course={course} ar={ar} current="groups" />
          {board && (
            <CourseGroups
              key={course.id}
              ar={ar}
              board={board}
              enrollments={courseEntries}
              disabled={busy || offline}
              move={(id, teacherId) =>
                void submit(
                  '/api/admin/enrollments/assignment',
                  { kind: 'group', courseId: course.id, ids: [id], teacherId },
                  undefined,
                  `group:${course.id}:${id}`,
                )
              }
            />
          )}
        </>
      )}
      {course && (query.courseView === 'students' || query.courseView === 'grades') && (
        <>
          {query.courseView === 'students' && (
            <CoursePageHeader
              course={course}
              ar={ar}
              current="students"
              actions={
                <div className="table-toolbar-actions">
                  <TableControls model={membershipTable} />
                </div>
              }
            />
          )}
          {query.courseView === 'grades' && !board && (
            <CoursePageHeader course={course} ar={ar} current="grades" />
          )}
          {query.courseView === 'students' && (
            <>
              <div className="course-student-tools flex flex-wrap items-end gap-3">
                <label>
                  {ar ? 'طريقة العرض' : 'View'}
                  <select
                    aria-label={ar ? 'عرض الطلاب' : 'Student view'}
                    value={query.studentMode}
                    onChange={(event) => setQuery({ studentMode: event.target.value })}
                  >
                    <option value="all">{ar ? 'كل الأعمدة' : 'All columns'}</option>
                    <option value="groups">{ar ? 'المجموعات' : 'Groups'}</option>
                    <option value="books">{ar ? 'الكتب' : 'Books'}</option>
                  </select>
                </label>
                <div className="flex min-w-0 flex-[1_0_10rem] items-end gap-2">
                  <label className="min-w-0 flex-1">
                    {ar ? 'البحث عن طالب' : 'Find a student'}
                    <input
                      type="search"
                      value={query.q}
                      onChange={(event) => setQuery({ q: event.target.value })}
                      dir="auto"
                    />
                  </label>
                  <button
                    type="button"
                    className="icon-button shrink-0"
                    aria-label={addStudentLabel}
                    title={addStudentLabel}
                    onClick={() => openChildView({ view: 'add-student', courseView: 'students' })}
                  >
                    <Plus size={22} aria-hidden="true" />
                  </button>
                </div>
              </div>
              {query.studentMode === 'groups' && (
                <div className="flex flex-wrap items-end gap-3">
                  <div className="flex flex-wrap items-center gap-3">
                    <button
                      className="secondary"
                      disabled={busy || !visibleIds.length}
                      onClick={() =>
                        setSelected((current) => ({
                          ...current,
                          [course.id]: [...new Set([...selectedIds, ...visibleIds])],
                        }))
                      }
                    >
                      {ar ? 'تحديد الظاهرين' : 'Select visible'}
                    </button>
                    <span role="status">
                      {ar ? `المحدد: ${selectedIds.length}` : `Selected: ${selectedIds.length}`}
                    </span>
                  </div>
                  {selectedIds.length > 0 && (
                    <>
                      <button
                        className="secondary"
                        disabled={busy || !selectedIds.length}
                        onClick={() => setSelected((current) => ({ ...current, [course.id]: [] }))}
                      >
                        {ar ? 'مسح التحديد' : 'Clear selection'}
                      </button>
                      <label>
                        {ar ? 'المعلم للمحدد' : 'Teacher for selected'}
                        <select
                          value={bulkTeacher}
                          onChange={(event) => setBulkTeacher(event.target.value)}
                        >
                          <option value="unchanged">{ar ? 'بدون تغيير' : 'Keep unchanged'}</option>
                          <option value="none">{ar ? 'بلا معلم' : 'No teacher'}</option>
                          {board?.teachers.map((teacher) => (
                            <option key={teacher.teacherId} value={teacher.teacherId}>
                              {teacher.name}
                            </option>
                          ))}
                        </select>
                      </label>
                      <button
                        className="primary"
                        disabled={
                          busy || offline || !selectedIds.length || bulkTeacher === 'unchanged'
                        }
                        onClick={() => {
                          const ids = selectedIds
                          const snapshot = drafts
                          const draftKeys = courseEntries
                            .filter((entry) => ids.includes(entry.id))
                            .map((entry) => `${course.id}:${entry.studentId}`)
                          void submit(
                            '/api/admin/enrollments/assignment',
                            {
                              kind: 'group',
                              courseId: course.id,
                              ids,
                              ...(bulkTeacher !== 'unchanged'
                                ? { teacherId: bulkTeacher === 'none' ? null : bulkTeacher }
                                : {}),
                            },
                            () => {
                              setDrafts((current) =>
                                Object.fromEntries(
                                  Object.entries(current).map(([key, draft]) => {
                                    if (!draftKeys.includes(key)) return [key, draft]
                                    const next = { ...draft }
                                    if (
                                      bulkTeacher !== 'unchanged' &&
                                      next.teacherId === snapshot[key]?.teacherId
                                    )
                                      delete next.teacherId
                                    return [key, next]
                                  }),
                                ),
                              )
                              setSelected((current) => ({
                                ...current,
                                [course.id]: (current[course.id] ?? []).filter(
                                  (id) => !ids.includes(id),
                                ),
                              }))
                            },
                            `group:${course.id}`,
                          )
                        }}
                      >
                        {ar ? 'تطبيق على المحددين' : 'Apply to selected'}
                      </button>
                    </>
                  )}
                </div>
              )}
              <div
                className="role-table-scroll"
                role="region"
                aria-label={ar ? 'طلاب الدورة' : 'Course students'}
                tabIndex={0}
              >
                <table
                  className={`mobile-record-table role-table course-students-table course-students-${query.studentMode}`}
                >
                  <TableHeader model={membershipTable} />
                  <tbody>
                    {membershipTable.rows.map((student) => {
                      const entry = courseEntries.find(
                        (item) => item.studentId === student.studentId,
                      )
                      const draftKey = `${course.id}:${student.studentId}`
                      const storedDraft = drafts[draftKey]
                      const draft = {
                        id: storedDraft?.id ?? entry?.id ?? '',
                        teacherId: storedDraft?.teacherId ?? entry?.teacherId ?? '',
                      }
                      const clearTeacherDraft = (value: string) => {
                        setDrafts((current) => {
                          const stored = current[draftKey]
                          if (!stored || stored.teacherId !== value) return current
                          const next = { ...stored }
                          delete next.teacherId
                          return { ...current, [draftKey]: next }
                        })
                      }
                      const saveMembership = (active: boolean) => {
                        if (!active && entry)
                          setSelected((current) => ({
                            ...current,
                            [course.id]: (current[course.id] ?? []).filter((id) => id !== entry.id),
                          }))
                        const id = draft.id || crypto.randomUUID()
                        if (!entry)
                          setDrafts((current) => ({
                            ...current,
                            [draftKey]: { ...current[draftKey], id },
                          }))
                        void submit(
                          entry
                            ? '/api/admin/enrollments/assignment'
                            : '/api/admin/enrollments/membership',
                          entry
                            ? { kind: 'edit', courseId: course.id, id, active }
                            : {
                                id,
                                courseId: course.id,
                                studentId: student.studentId,
                                teacherId: draft.teacherId || null,
                                active,
                              },
                          entry ? undefined : () => clearTeacherDraft(draft.teacherId),
                          `membership:${draftKey}`,
                          { membership: { key: draftKey, active } },
                        )
                      }
                      return (
                        <tr key={student.studentId}>
                          <td data-label={ar ? 'الاسم الأول' : 'First name'}>
                            {query.studentMode === 'groups' && entry?.active ? (
                              <div className="course-student-check course-student-name">
                                <input
                                  type="checkbox"
                                  aria-label={`${ar ? 'تحديد' : 'Select'}: ${student.firstName} ${student.lastName}`}
                                  checked={selectedIds.includes(entry.id)}
                                  onChange={(event) => {
                                    const checked = event.target.checked
                                    setSelected((current) => {
                                      const ids = current[course.id] ?? []
                                      return {
                                        ...current,
                                        [course.id]: checked
                                          ? [...ids, entry.id]
                                          : ids.filter((id) => id !== entry.id),
                                      }
                                    })
                                  }}
                                />
                                <a
                                  className="course-name-link"
                                  href={`?${new URLSearchParams({ page: 'app', view: 'student', student: student.studentId, course: course.id, courseView: 'students' })}`}
                                  onClick={(event) => {
                                    event.preventDefault()
                                    openChildView({ view: 'student', student: student.studentId })
                                  }}
                                >
                                  <bdi>{student.firstName}</bdi>
                                </a>
                              </div>
                            ) : (
                              <a
                                className="course-name-link"
                                href={`?${new URLSearchParams({ page: 'app', view: 'student', student: student.studentId, course: course.id, courseView: 'students' })}`}
                                onClick={(event) => {
                                  event.preventDefault()
                                  openChildView({ view: 'student', student: student.studentId })
                                }}
                              >
                                <bdi>{student.firstName}</bdi>
                              </a>
                            )}
                          </td>
                          <td dir="auto" data-label={ar ? 'اسم العائلة' : 'Last name'}>
                            {student.lastName || '—'}
                          </td>
                          {query.studentMode !== 'books' && (
                            <td data-label={ar ? 'مستوى الدورة' : 'Course level'}>
                              <bdi>{formatLevel(entry ? entry.level : student.group) || '—'}</bdi>
                            </td>
                          )}
                          {query.studentMode !== 'books' && (
                            <td
                              className="course-student-teacher"
                              data-label={ar ? 'المعلم' : 'Teacher'}
                            >
                              <select
                                aria-label={`${ar ? 'المعلم' : 'Teacher'}: ${student.firstName} ${student.lastName}`}
                                value={draft.teacherId}
                                disabled={offline || (busy && !entry)}
                                onChange={(event) => {
                                  const teacherId = event.target.value
                                  setDrafts((current) => ({
                                    ...current,
                                    [draftKey]: { ...current[draftKey], teacherId },
                                  }))
                                  if (entry)
                                    void submit(
                                      '/api/admin/enrollments/assignment',
                                      {
                                        kind: 'edit',
                                        courseId: course.id,
                                        id: entry.id,
                                        teacherId: teacherId || null,
                                      },
                                      () => clearTeacherDraft(teacherId),
                                      `teacher:${draftKey}`,
                                    )
                                }}
                              >
                                <option value="">{ar ? 'بلا معلم' : 'No teacher'}</option>
                                {board?.teachers.map((teacher) => (
                                  <option key={teacher.teacherId} value={teacher.teacherId}>
                                    {teacher.name}
                                  </option>
                                ))}
                              </select>
                            </td>
                          )}
                          <td data-label={ar ? 'مسجل' : 'Enrolled'}>
                            <input
                              type="checkbox"
                              role="switch"
                              aria-label={`${ar ? 'مسجل' : 'Enrolled'}: ${student.firstName} ${student.lastName}`}
                              checked={pendingMembership[draftKey] ?? entry?.active ?? false}
                              disabled={busy || offline}
                              onChange={(event) => saveMembership(event.target.checked)}
                            />
                          </td>
                          {query.studentMode !== 'groups' && (
                            <td data-label={ar ? 'استلم الكتاب' : 'Handbook received'}>
                              <label className="inline-flex size-11 items-center justify-center align-middle">
                                <input
                                  type="checkbox"
                                  aria-label={`${ar ? 'استلم الكتاب' : 'Handbook received'}: ${student.firstName} ${student.lastName}`}
                                  checked={entry?.handbookReceived ?? false}
                                  disabled={busy || offline || !entry}
                                  onChange={(event) =>
                                    entry &&
                                    void submit(
                                      '/api/admin/enrollments/handbook',
                                      { id: entry.id, handbookReceived: event.target.checked },
                                      undefined,
                                      `handbook:${draftKey}`,
                                    )
                                  }
                                />
                              </label>
                            </td>
                          )}
                        </tr>
                      )
                    })}
                  </tbody>
                </table>
              </div>
            </>
          )}
          {query.courseView === 'grades' && board && (
            <GradePage
              key={course.id}
              userId={userId}
              refreshAfterWrite={refreshAfterWrite}
              course={course}
              entries={gradeEnrollments.filter((item) => item.courseId === course.id)}
              exportEntries={courseEntries}
              students={board.students}
              accounts={board.accounts}
              events={events}
              busy={busy}
              offline={offline}
              ar={ar}
              save={submit}
            />
          )}
        </>
      )}
    </section>
  )
}

type GradeDraft = {
  practical?: { original: number | null; value: string }
  theory?: { original: number | null; value: string }
  notes?: { original: string; value: string }
}

function GradePage({
  userId,
  refreshAfterWrite,
  course,
  entries,
  exportEntries,
  students,
  accounts,
  events,
  busy,
  offline,
  ar,
  save,
}: {
  userId: string
  refreshAfterWrite: () => Promise<boolean>
  course: Course
  entries: Enrollment[]
  exportEntries: Enrollment[]
  students: Board['students']
  accounts: Board['accounts']
  events: LedgerEvent[]
  busy: boolean
  offline: boolean
  ar: boolean
  save: (path: string, body: unknown) => Promise<boolean>
}) {
  const [drafts, setDrafts] = useState<Record<string, GradeDraft>>({})
  const [persisting, setPersisting] = useState<Record<string, boolean>>({})
  const [saveError, setSaveError] = useState(false)
  const pendingGrades = useGradeQueue(userId)
  useEffect(() => {
    function accepted(event: Event) {
      if (event instanceof CustomEvent && event.detail === gradeScope(userId)?.generation)
        void refreshAfterWrite()
    }
    window.addEventListener(gradeAcceptedEvent, accepted)
    return () => window.removeEventListener(gradeAcceptedEvent, accepted)
  }, [refreshAfterWrite, userId])
  async function saveGrades(entry: Enrollment, label: string) {
    const scope = gradeScope(userId)
    if (!scope) {
      setSaveError(true)
      return
    }
    const raw = drafts[entry.id]
    if (!raw) return
    const previous: GradeValues = {
      practical: raw.practical ? raw.practical.original : entry.practical,
      theory: raw.theory ? raw.theory.original : entry.theory,
      notes: raw.notes?.original ?? entry.notes,
    }
    const next: GradeValues = {
      practical: raw.practical
        ? raw.practical.value === ''
          ? null
          : Number(raw.practical.value)
        : entry.practical,
      theory: raw.theory
        ? raw.theory.value === ''
          ? null
          : Number(raw.theory.value)
        : entry.theory,
      notes: raw.notes?.value ?? entry.notes,
    }
    setPersisting((current) => ({ ...current, [entry.id]: true }))
    setSaveError(false)
    try {
      await queueGradeEdit({ scope, id: entry.id, label, previous, next })
      setDrafts((current) => {
        const updated = { ...current }
        delete updated[entry.id]
        return updated
      })
    } catch {
      setSaveError(true)
    } finally {
      setPersisting((current) => ({ ...current, [entry.id]: false }))
    }
  }
  const [decisionEntry, setDecisionEntry] = useState<string | null>(null)
  const [decisionError, setDecisionError] = useState(false)
  const dialogRef = useRef<HTMLDialogElement>(null)
  useEffect(() => {
    if (decisionEntry) dialogRef.current?.showModal()
  }, [decisionEntry])
  const { missingScores: showMissing } = useQueryState()
  const missing = missingCourseScores(entries)
  const [exportOpen, setExportOpen] = useState(false)
  const [decisions, setDecisions] = useState<
    Record<string, { requestId: string; result: 'pass' | 'fail' | 'calculated'; note: string }>
  >({})
  const [settings, setSettings] = useState({
    attendanceMinimum: String(course.attendanceMinimum),
    practicalMinimum: String(course.practicalMinimum),
    theoryMinimum: String(course.theoryMinimum),
  })
  const settingsDirty =
    Number(settings.attendanceMinimum) !== course.attendanceMinimum ||
    Number(settings.practicalMinimum) !== course.practicalMinimum ||
    Number(settings.theoryMinimum) !== course.theoryMinimum ||
    Object.values(settings).some((value) => value === '')
  const gradePending = (id: string) => pendingGrades.find((item) => item.operation.id === id)
  const dirtyGrade = (entry: Enrollment) => {
    const draft = drafts[entry.id]
    return (
      gradePending(entry.id) ||
      (draft?.practical &&
        draft.practical.value !== (draft.practical.original?.toString() ?? '')) ||
      (draft?.theory && draft.theory.value !== (draft.theory.original?.toString() ?? '')) ||
      (draft?.notes && draft.notes.value !== draft.notes.original)
    )
  }
  const gradeDirty = entries.some(dirtyGrade)
  const attendance = projectEvents(events).attendance
  const gradeResult = (entry: Enrollment, ratio: number | null) => {
    if (settingsDirty || dirtyGrade(entry)) return 'pending'
    return savedGradeResult(course, entry, ratio)
  }
  const attendanceRatio = (entry: Enrollment) => courseAttendance(attendance, entry).ratio
  const resultLabel = (result: string) =>
    result === 'pending'
      ? ar
        ? 'قيد الانتظار'
        : 'Pending'
      : result === 'pass'
        ? ar
          ? 'ناجح'
          : 'Pass'
        : ar
          ? 'راسب'
          : 'Fail'
  const gradeTable = useDataTable(
    'grades',
    showMissing ? missing : entries,
    [
      {
        id: 'first',
        label: ar ? 'الاسم الأول' : 'First name',
        value: (entry) => students.find((s) => s.studentId === entry.studentId)?.firstName,
      },
      {
        id: 'last',
        label: ar ? 'اسم العائلة' : 'Last name',
        value: (entry) => students.find((s) => s.studentId === entry.studentId)?.lastName,
      },
      {
        id: 'level',
        label: ar ? 'مستوى الدورة' : 'Course level',
        value: (entry) => formatLevel(entry.level),
      },
      {
        id: 'attendance',
        label: ar ? 'الحضور' : 'Attendance',
        value: attendanceRatio,
        filterText: (entry) => {
          const ratio = attendanceRatio(entry)
          return ratio === null ? (ar ? 'غير متاح' : 'Unavailable') : `${ratio.toFixed(1)}%`
        },
      },
      ...(['theory', 'practical'] as const).map((key) => ({
        id: key,
        label: key === 'practical' ? (ar ? 'عملي' : 'Practical') : ar ? 'نظري' : 'Theory',
        value: (entry: Enrollment) => {
          const pending = gradePending(entry.id)?.operation.fields[key]
          const value = drafts[entry.id]?.[key]?.value ?? (pending ? pending.next : entry[key])
          return value == null || value === '' ? null : Number(value)
        },
      })),
      {
        id: 'notes',
        label: ar ? 'ملاحظة أداء الدورة' : 'Course performance note',
        value: (entry) =>
          drafts[entry.id]?.notes?.value ??
          gradePending(entry.id)?.operation.fields.notes?.next ??
          entry.notes,
      },
      {
        id: 'result',
        choices: ar ? ['قيد الانتظار', 'ناجح', 'راسب'] : ['Pending', 'Pass', 'Fail'],
        label: ar ? 'النتيجة' : 'Result',
        value: (entry) => {
          const last = entry.gradeDecisions.at(-1)
          const result =
            last && last.result !== 'calculated'
              ? last.result
              : gradeResult(entry, attendanceRatio(entry))
          const average =
            entry.practical === null || entry.theory === null
              ? -1
              : (entry.practical + entry.theory) / 2
          // Include the missing-score sentinel (-1) in each separate status band.
          return (result === 'pass' ? 204 : result === 'fail' ? 102 : 0) + average
        },
      },
    ],
    false,
  )
  return (
    <>
      <CoursePageHeader
        course={course}
        ar={ar}
        current="grades"
        actions={
          <div className="table-toolbar-actions">
            <button
              className="secondary flex items-center gap-2"
              aria-label={ar ? 'تصدير Excel' : 'Export Excel'}
              data-diagnostic-action="export"
              title={ar ? 'تصدير Excel' : 'Export Excel'}
              disabled={busy || offline || settingsDirty || !!gradeDirty}
              onClick={() => setExportOpen(true)}
            >
              <Download size={20} aria-hidden="true" />
              <span className="hidden sm:inline">{ar ? 'تصدير Excel' : 'Export Excel'}</span>
            </button>
            <TableControls model={gradeTable} />
          </div>
        }
      />
      {exportOpen && (
        <CourseExportDialog
          course={course}
          entries={exportEntries}
          students={students}
          attendance={attendance}
          ar={ar}
          onClose={() => setExportOpen(false)}
        />
      )}
      <div className="flex flex-wrap items-end gap-3 ltr:flex-row-reverse">
        <form
          className="ml-auto flex flex-wrap items-end justify-end gap-3"
          dir="ltr"
          onSubmit={(event) => {
            event.preventDefault()
            void save('/api/admin/courses/settings', {
              id: course.id,
              attendanceMinimum: Number(settings.attendanceMinimum),
              practicalMinimum: Number(settings.practicalMinimum),
              theoryMinimum: Number(settings.theoryMinimum),
            })
          }}
        >
          {(
            [
              ['attendanceMinimum', ar ? 'الحد الأدنى للحضور %' : 'Minimum attendance %'],
              ['practicalMinimum', ar ? 'درجة النجاح العملي' : 'Practical passing grade'],
              ['theoryMinimum', ar ? 'درجة النجاح النظري' : 'Theory passing grade'],
            ] as const
          ).map(([key, label]) => (
            <label className="w-full max-w-48" key={key} dir={ar ? 'rtl' : 'ltr'}>
              {label}
              <input
                type="number"
                disabled={busy}
                min="0"
                max="100"
                step="any"
                required
                value={settings[key]}
                onChange={(event) =>
                  setSettings((current) => ({ ...current, [key]: event.target.value }))
                }
              />
            </label>
          ))}
          <button className="secondary" disabled={busy || offline || !settingsDirty}>
            {ar ? 'حفظ الحدود' : 'Save thresholds'}
          </button>
        </form>
        {(missing.length > 0 || showMissing) && (
          <button
            type="button"
            className="secondary mr-auto"
            aria-pressed={showMissing}
            onClick={() => setQuery({ missing: showMissing ? '' : '1' })}
          >
            {ar
              ? `إظهار الدرجات الناقصة (${missing.length})`
              : `Show missing scores (${missing.length})`}
          </button>
        )}
      </div>
      <p>
        {ar
          ? 'الحضور = الحاضر / (الحاضر + الغائب + المعذور). الأيام غير المسجلة مستبعدة.'
          : 'Attendance = Present / (Present + Absent + Excused). Unmarked days are excluded.'}
      </p>
      {saveError && (
        <p className="form-error" role="alert">
          {ar
            ? 'تعذر حفظ تغييرات الدرجات على هذا الجهاز.'
            : 'Unable to save grade changes on this device.'}
        </p>
      )}
      <div
        className="role-table-scroll grade-table-scroll"
        role="region"
        aria-label={ar ? 'درجات الدورة' : 'Course grades'}
        tabIndex={0}
      >
        <table className="mobile-record-table role-table min-w-[68rem] [&_th]:whitespace-nowrap [&_input]:w-24 [&_td]:align-top">
          <TableHeader model={gradeTable} actions />
          <tbody>
            {gradeTable.rows.map((entry) => {
              const student = students.find((item) => item.studentId === entry.studentId)
              if (!student) return null
              const pending = gradePending(entry.id)
              const displayed = pending ? applyGradeChanges(entry, pending.operation.fields) : entry
              const raw = drafts[entry.id]
              const draft = {
                practical: raw?.practical?.value ?? displayed.practical?.toString() ?? '',
                theory: raw?.theory?.value ?? displayed.theory?.toString() ?? '',
                notes: raw?.notes?.value ?? displayed.notes,
              }
              const gradeLocked = busy || !!persisting[entry.id] || !!pending
              const ratio = attendanceRatio(entry)
              const result = gradeResult(entry, ratio)
              const lastDecision = entry.gradeDecisions.at(-1)
              const effective =
                lastDecision && lastDecision.result !== 'calculated' ? lastDecision.result : result
              const decisionDraft = decisions[entry.id] ?? {
                requestId: crypto.randomUUID(),
                result: 'pass' as const,
                note: '',
              }
              const formId = `grades-${entry.id}`
              return (
                <tr key={entry.id} className={`grade-row-${effective}`}>
                  <td dir="auto" data-label={ar ? 'الاسم الأول' : 'First name'}>
                    <a
                      className="course-name-link"
                      href={`?${new URLSearchParams({ page: 'app', view: 'student', student: student.studentId, course: course.id, courseView: 'grades' })}`}
                      onClick={(event) => {
                        event.preventDefault()
                        openChildView({ view: 'student', student: student.studentId })
                      }}
                    >
                      {student.firstName}
                    </a>
                  </td>
                  <td dir="auto" data-label={ar ? 'اسم العائلة' : 'Last name'}>
                    {student.lastName || '—'}
                  </td>
                  <td data-label={ar ? 'مستوى الدورة' : 'Course level'}>
                    <bdi>{formatLevel(entry.level) || '—'}</bdi>
                  </td>
                  <td data-label={ar ? 'الحضور' : 'Attendance'}>
                    <span
                      className={
                        !settingsDirty && ratio !== null
                          ? ratio >= course.attendanceMinimum
                            ? 'grade-pass'
                            : 'grade-fail'
                          : undefined
                      }
                    >
                      {ratio === null ? (ar ? 'غير متاح' : 'Unavailable') : `${ratio.toFixed(1)}%`}
                    </span>
                  </td>
                  {(['theory', 'practical'] as const).map((key) => (
                    <td
                      key={key}
                      data-label={
                        key === 'practical' ? (ar ? 'عملي' : 'Practical') : ar ? 'نظري' : 'Theory'
                      }
                    >
                      <input
                        form={formId}
                        className={
                          !settingsDirty &&
                          entry[key] !== null &&
                          draft[key] === entry[key].toString()
                            ? entry[key] >=
                              (key === 'practical' ? course.practicalMinimum : course.theoryMinimum)
                              ? 'grade-pass'
                              : 'grade-fail'
                            : undefined
                        }
                        aria-label={`${key === 'practical' ? (ar ? 'عملي' : 'Practical') : ar ? 'نظري' : 'Theory'}: ${student.firstName} ${student.lastName}`}
                        type="number"
                        disabled={gradeLocked}
                        min="0"
                        max="100"
                        step="any"
                        value={draft[key]}
                        onChange={(event) => {
                          const value = event.target.value
                          setDrafts((current) => {
                            const existing = current[entry.id]
                            return {
                              ...current,
                              [entry.id]: {
                                ...existing,
                                [key]: {
                                  value,
                                  original: existing?.[key] ? existing[key].original : entry[key],
                                },
                              },
                            }
                          })
                        }}
                      />
                    </td>
                  ))}
                  <td
                    className="grade-notes"
                    data-label={ar ? 'ملاحظة أداء الدورة' : 'Course performance note'}
                  >
                    <textarea
                      form={formId}
                      className="resize-y min-[1025px]:min-w-48"
                      aria-label={`${ar ? 'ملاحظة أداء الدورة' : 'Course performance note'}: ${student.firstName} ${student.lastName}`}
                      disabled={gradeLocked}
                      value={draft.notes}
                      onChange={(event) => {
                        const value = event.target.value
                        setDrafts((current) => ({
                          ...current,
                          [entry.id]: {
                            ...current[entry.id],
                            notes: {
                              value,
                              original: current[entry.id]?.notes?.original ?? entry.notes,
                            },
                          },
                        }))
                      }}
                    />
                  </td>
                  <td data-label={ar ? 'النتيجة' : 'Result'}>
                    <div>
                      {lastDecision && lastDecision.result !== 'calculated' && (
                        <>{ar ? 'المحسوبة' : 'Calculated'}: </>
                      )}
                      <span className={result === 'pending' ? undefined : `grade-${result}`}>
                        {resultLabel(result)}
                      </span>
                    </div>
                    {lastDecision && lastDecision.result !== 'calculated' && (
                      <div>
                        {ar ? 'النهائية' : 'Effective'}:{' '}
                        <span className={`grade-${effective}`}>{resultLabel(effective)}</span>
                      </div>
                    )}
                  </td>
                  <td className="grade-actions" data-label={ar ? 'الإجراءات' : 'Actions'}>
                    <form
                      id={formId}
                      onSubmit={(event) => {
                        event.preventDefault()
                        if (!gradeLocked)
                          void saveGrades(
                            entry,
                            `${course.name} · ${student.firstName} ${student.lastName}`,
                          )
                      }}
                    >
                      <button
                        className="icon-button gap-2"
                        disabled={gradeLocked}
                        aria-label={ar ? 'حفظ الدرجات' : 'Save grades'}
                        title={ar ? 'حفظ الدرجات' : 'Save grades'}
                      >
                        <Save size={20} className="shrink-0" aria-hidden="true" />
                        <span>{ar ? 'حفظ الدرجات' : 'Save grades'}</span>
                      </button>
                    </form>
                    <RowActions
                      name={`${student.firstName} ${student.lastName}`}
                      ar={ar}
                      actions={[
                        {
                          label: ar ? 'تعديل النتيجة' : 'Override result',
                          disabled: busy,
                          onClick: () => {
                            setDecisionError(false)
                            setDecisionEntry(entry.id)
                          },
                        },
                      ]}
                    />
                    {decisionEntry === entry.id && (
                      <dialog
                        ref={dialogRef}
                        className="grade-dialog"
                        aria-labelledby={`decision-title-${entry.id}`}
                        onClose={() => setDecisionEntry(null)}
                        onCancel={(event) => {
                          if (busy) event.preventDefault()
                        }}
                      >
                        <h2 id={`decision-title-${entry.id}`}>
                          {ar ? 'تعديل النتيجة' : 'Override result'}: {student.firstName}{' '}
                          {student.lastName}
                        </h2>
                        <form
                          onSubmit={(event) => {
                            event.preventDefault()
                            setDecisions((current) => ({ ...current, [entry.id]: decisionDraft }))
                            void save('/api/admin/enrollments/decision', {
                              id: entry.id,
                              ...decisionDraft,
                            }).then((ok) => {
                              if (!ok) {
                                setDecisionError(true)
                                return
                              }
                              dialogRef.current?.close()
                              setDecisions((current) => {
                                const next = { ...current }
                                delete next[entry.id]
                                return next
                              })
                            })
                          }}
                        >
                          <label>
                            {ar ? 'القرار' : 'Decision'}
                            <select
                              value={decisionDraft.result}
                              disabled={busy}
                              onChange={(event) => {
                                const result = event.target.value
                                if (
                                  result === 'pass' ||
                                  result === 'fail' ||
                                  result === 'calculated'
                                )
                                  setDecisions((current) => ({
                                    ...current,
                                    [entry.id]: { ...decisionDraft, result },
                                  }))
                              }}
                            >
                              <option value="pass">{ar ? 'ناجح' : 'Pass'}</option>
                              <option value="fail">{ar ? 'راسب' : 'Fail'}</option>
                              <option value="calculated">
                                {ar ? 'استعادة النتيجة المحسوبة' : 'Restore calculated result'}
                              </option>
                            </select>
                          </label>
                          <label>
                            {ar ? 'سبب القرار (مطلوب)' : 'Decision note (required)'}
                            <textarea
                              required
                              maxLength={2000}
                              disabled={busy}
                              value={decisionDraft.note}
                              onChange={(event) =>
                                setDecisions((current) => ({
                                  ...current,
                                  [entry.id]: { ...decisionDraft, note: event.target.value },
                                }))
                              }
                            />
                          </label>
                          {decisionError && (
                            <p role="alert">
                              {ar
                                ? 'تعذر حفظ القرار. حاول مرة أخرى.'
                                : 'Could not save decision. Try again.'}
                            </p>
                          )}
                          <button
                            className="secondary"
                            disabled={busy || offline || !decisionDraft.note.trim()}
                          >
                            {ar ? 'حفظ القرار' : 'Save decision'}
                          </button>
                        </form>
                        {entry.gradeDecisions.length > 0 && (
                          <details className="decision-history">
                            <summary>
                              {ar ? 'سجل القرارات' : 'Decision history'} (
                              {entry.gradeDecisions.length})
                            </summary>
                            <ul>
                              {[...entry.gradeDecisions].reverse().map((item) => {
                                const actor = accounts.find(
                                  (account) => account.authUserId === item.actor,
                                )
                                return (
                                  <li key={item.requestId}>
                                    <div className="decision-history-heading">
                                      <strong
                                        className={
                                          item.result === 'calculated'
                                            ? undefined
                                            : `grade-${item.result}`
                                        }
                                      >
                                        {item.result === 'calculated'
                                          ? ar
                                            ? 'المحسوبة'
                                            : 'Calculated'
                                          : resultLabel(item.result)}
                                      </strong>
                                      <time dateTime={item.at}>
                                        {new Intl.DateTimeFormat(ar ? 'ar' : 'en', {
                                          dateStyle: 'medium',
                                          timeStyle: 'short',
                                        }).format(new Date(item.at))}
                                      </time>
                                    </div>
                                    <p dir="auto">{item.note}</p>
                                    {actor ? (
                                      <a
                                        className="decision-history-actor"
                                        dir="auto"
                                        href={`?${new URLSearchParams({ page: 'app', view: 'settings', account: actor.authUserId })}`}
                                      >
                                        {accountProfileLabel(actor)}
                                      </a>
                                    ) : (
                                      <span>
                                        {ar
                                          ? 'اسم المسؤول غير متاح'
                                          : 'Administrator name unavailable'}
                                      </span>
                                    )}
                                  </li>
                                )
                              })}
                            </ul>
                          </details>
                        )}
                        <button
                          type="button"
                          className="secondary"
                          disabled={busy}
                          onClick={() => dialogRef.current?.close()}
                        >
                          {ar ? 'إلغاء' : 'Cancel'}
                        </button>
                      </dialog>
                    )}
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>
      {!course.open && <p role="status">{ar ? 'الدورة مغلقة' : 'Course closed'}</p>}
      {course.open && (
        <button
          className="primary"
          disabled={busy || offline || settingsDirty || !!gradeDirty || missing.length > 0}
          onClick={() => void save('/api/admin/courses/status', { id: course.id, open: false })}
        >
          {ar ? 'إغلاق الدورة' : 'Close course'}
        </button>
      )}
    </>
  )
}
