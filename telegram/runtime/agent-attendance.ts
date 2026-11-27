import { z } from 'zod'
import { previewAgentWrite, validateAgentCall, type AgentTool } from './agent-tools'

type Language = 'en' | 'ar'
type Request = {
  student: string
  course: string
  date: string
  status: 'present' | 'absent' | 'excused'
}
type Plan =
  | { kind: 'approval'; text: string; tool: string; args: Record<string, unknown> }
  | { kind: 'answer'; text: string }
const courseSchema = z.object({ id: z.uuid(), name: z.string().min(1).max(200), open: z.boolean() })
const sessionSchema = z.object({
  course: courseSchema,
  date: z.iso.date(),
  session: z
    .array(
      z.object({
        studentId: z.uuid(),
        enrollmentId: z.uuid(),
        name: z.string().min(1).max(200),
        status: z.enum(['present', 'absent', 'excused', 'unmarked']),
        eventId: z.uuid().nullable(),
      }),
    )
    .max(10000),
})
const normalize = (value: string) =>
  value.normalize('NFKC').toLocaleLowerCase().replace(/\s+/g, ' ').trim()

const plannedRecords = z.object({
  records: z
    .array(
      z.object({
        expectedEventId: z.uuid().nullable(),
        event: z.object({
          payload: z.object({ studentId: z.uuid(), enrollmentId: z.uuid(), date: z.iso.date() }),
        }),
      }),
    )
    .min(1),
})

export function matchesAttendanceSession(args: unknown, source: unknown) {
  const proposed = plannedRecords.safeParse(args)
  const session = sessionSchema.safeParse(source)
  if (!proposed.success || !session.success || !session.data.course.open) return false
  const targets = new Set<string>()
  for (const {
    expectedEventId,
    event: { payload },
  } of proposed.data.records) {
    if (targets.has(payload.enrollmentId) || payload.date !== session.data.date) return false
    targets.add(payload.enrollmentId)
    const [row, duplicate] = session.data.session.filter(
      (item) => item.studentId === payload.studentId && item.enrollmentId === payload.enrollmentId,
    )
    if (!row || duplicate || row.eventId !== expectedEventId) return false
  }
  return true
}

function parse(question: string): Request | 'missing-date' | null {
  const en = question.match(
    /^mark\s+(.+?)\s+(present|absent|excused)\s+in\s+(.+?)(?:\s+on\s+(\d{4}-\d{2}-\d{2}))?[.!]?$/i,
  )
  const ar = question.match(
    /^(?:\u0633\u062c\u0644|\u0633\u062c\u0651\u0644|\u062d\u0637)\s+(.+?)\s+(\u062d\u0627\u0636\u0631|\u062d\u0627\u0636\u0631\u0629|\u063a\u0627\u0626\u0628|\u063a\u0627\u0626\u0628\u0629|\u063a\u0627\u064a\u0628|\u0645\u0639\u0630\u0648\u0631)\s+\u0641\u064a\s+(?:\u062f\u0648\u0631\u0629\s+)?(.+?)(?:\s+(?:\u0641\u064a|\u0628\u062a\u0627\u0631\u064a\u062e|\u064a\u0648\u0645)\s+(\d{4}-\d{2}-\d{2}))?[.!\u061f]?$/u,
  )
  const match = en ?? ar
  if (!match) return null
  const [, student, rawStatus, course, date] = match
  if (!date || !z.iso.date().safeParse(date).success) return 'missing-date'
  const status = /^(present|\u062d\u0627\u0636\u0631\u0629?)$/iu.test(rawStatus)
    ? 'present'
    : /^(absent|\u063a\u0627\u0626\u0628\u0629?|\u063a\u0627\u064a\u0628)$/iu.test(rawStatus)
      ? 'absent'
      : 'excused'
  return { student: student.trim(), course: course.trim(), date, status }
}

export async function planAttendance(
  question: string,
  lang: Language,
  tools: readonly AgentTool[],
  call: (name: string, args: Record<string, unknown>) => Promise<unknown>,
): Promise<Plan | null> {
  const request = parse(question)
  if (!request) return null
  const fail = (en: string, ar: string): Plan => ({ kind: 'answer', text: lang === 'ar' ? ar : en })
  if (request === 'missing-date')
    return fail(
      'No change saved. Give the date as YYYY-MM-DD.',
      '\u0644\u0645 \u064a\u064f\u062d\u0641\u0638 \u0623\u064a \u062a\u063a\u064a\u064a\u0631. \u0623\u062f\u062e\u0644 \u0627\u0644\u062a\u0627\u0631\u064a\u062e \u0628\u0635\u064a\u063a\u0629 YYYY-MM-DD.',
    )
  const list = tools.find((tool) => tool.name === 'list_courses' && tool.kind === 'read')
  const view = tools.find(
    (tool) => tool.name === 'get_course_attendance_view' && tool.kind === 'read',
  )
  const sync = tools.find((tool) => tool.name === 'sync_attendance' && tool.kind === 'write')
  if (!list || !view || !sync)
    return fail(
      'No change saved. Attendance access is unavailable; use /menu.',
      '\u0644\u0645 \u064a\u064f\u062d\u0641\u0638 \u0623\u064a \u062a\u063a\u064a\u064a\u0631. \u0635\u0644\u0627\u062d\u064a\u0629 \u0627\u0644\u062d\u0636\u0648\u0631 \u063a\u064a\u0631 \u0645\u062a\u0627\u062d\u0629\u061b \u0627\u0633\u062a\u062e\u062f\u0645 /menu.',
    )
  try {
    const courses = z
      .array(courseSchema)
      .max(10000)
      .parse(await call(list.name, validateAgentCall(list, {})))
    const matches = courses.filter((course) => normalize(course.name) === normalize(request.course))
    if (matches.length !== 1)
      return fail(
        'No change saved. The course is missing or ambiguous; provide its exact name.',
        '\u0644\u0645 \u064a\u064f\u062d\u0641\u0638 \u0623\u064a \u062a\u063a\u064a\u064a\u0631. \u0627\u0644\u062f\u0648\u0631\u0629 \u063a\u064a\u0631 \u0645\u0648\u062c\u0648\u062f\u0629 \u0623\u0648 \u0627\u0633\u0645\u0647\u0627 \u063a\u064a\u0631 \u0645\u062d\u062f\u062f\u061b \u0623\u062f\u062e\u0644 \u0627\u0633\u0645\u0647\u0627 \u0627\u0644\u0643\u0627\u0645\u0644.',
      )
    const course = matches[0]
    if (!course.open)
      return fail(
        'No change saved. This course is closed.',
        '\u0644\u0645 \u064a\u064f\u062d\u0641\u0638 \u0623\u064a \u062a\u063a\u064a\u064a\u0631. \u0647\u0630\u0647 \u0627\u0644\u062f\u0648\u0631\u0629 \u0645\u063a\u0644\u0642\u0629.',
      )
    const viewArgs = validateAgentCall(view, {
      courseId: course.id,
      date: request.date,
      view: 'session',
      language: lang,
    })
    const session = sessionSchema.parse(await call(view.name, viewArgs))
    if (
      session.course.id !== course.id ||
      session.course.name !== course.name ||
      !session.course.open ||
      session.date !== request.date
    )
      throw new Error('Session changed')
    const students = session.session.filter(
      (student) => normalize(student.name) === normalize(request.student),
    )
    if (students.length !== 1)
      return fail(
        'No change saved. The student is missing or ambiguous in this course; provide the exact name.',
        '\u0644\u0645 \u064a\u064f\u062d\u0641\u0638 \u0623\u064a \u062a\u063a\u064a\u064a\u0631. \u0627\u0644\u0637\u0627\u0644\u0628 \u063a\u064a\u0631 \u0645\u0648\u062c\u0648\u062f \u0623\u0648 \u0627\u0633\u0645\u0647 \u063a\u064a\u0631 \u0645\u062d\u062f\u062f \u0641\u064a \u0627\u0644\u062f\u0648\u0631\u0629\u061b \u0623\u062f\u062e\u0644 \u0627\u0644\u0627\u0633\u0645 \u0627\u0644\u0643\u0627\u0645\u0644.',
      )
    const student = students[0]
    if (student.status !== 'unmarked' && student.eventId === null)
      throw new Error('Inconsistent mark')
    const args = validateAgentCall(sync, {
      records: [
        {
          event: {
            id: crypto.randomUUID(),
            clientTime: new Date().toISOString(),
            payload: {
              kind: 'attendance',
              studentId: student.studentId,
              enrollmentId: student.enrollmentId,
              date: request.date,
              status: request.status,
            },
          },
          expectedEventId: student.eventId,
        },
      ],
    })
    const labels = {
      [student.studentId]: student.name,
      [student.enrollmentId]: `${student.name} · ${course.name}`,
    }
    return {
      kind: 'approval',
      text: previewAgentWrite(sync, args, lang, labels),
      tool: sync.name,
      args,
    }
  } catch {
    return fail(
      'No change saved. Could not verify the current attendance; try again or use /menu.',
      '\u0644\u0645 \u064a\u064f\u062d\u0641\u0638 \u0623\u064a \u062a\u063a\u064a\u064a\u0631. \u062a\u0639\u0630\u0631 \u0627\u0644\u062a\u062d\u0642\u0642 \u0645\u0646 \u0627\u0644\u062d\u0636\u0648\u0631\u061b \u062d\u0627\u0648\u0644 \u0645\u062c\u062f\u062f\u0627\u064b \u0623\u0648 \u0627\u0633\u062a\u062e\u062f\u0645 /menu.',
    )
  }
}

export function renderAttendance(value: unknown, lang: Language): string | null {
  const parsed = z
    .object({
      course: z.object({ name: z.string().max(200) }).nullable(),
      date: z.iso.date(),
      session: z
        .array(
          z.object({
            name: z.string().max(200),
            status: z.enum(['present', 'absent', 'excused', 'unmarked']),
          }),
        )
        .max(10000),
    })
    .safeParse(value)
  if (!parsed.success) return null
  const statuses =
    lang === 'ar'
      ? {
          present: '\u062d\u0627\u0636\u0631',
          absent: '\u063a\u0627\u0626\u0628',
          excused: '\u0645\u0639\u0630\u0648\u0631',
          unmarked: '\u0644\u0645 \u064a\u064f\u0633\u062c\u0644',
        }
      : { present: 'Present', absent: 'Absent', excused: 'Excused', unmarked: 'Unmarked' }
  const data = parsed.data
  const header = [data.course?.name, data.date].filter(Boolean).join(' — ')
  const lines = data.session.slice(0, 30).map((row) => `${row.name}: ${statuses[row.status]}`)
  if (!lines.length)
    lines.push(
      lang === 'ar'
        ? '\u0644\u0627 \u064a\u0648\u062c\u062f \u0637\u0644\u0627\u0628.'
        : 'No students in this session.',
    )
  if (data.session.length > 30)
    lines.push(
      lang === 'ar'
        ? '\u0627\u0641\u062a\u062d /menu \u0644\u0639\u0631\u0636 \u0627\u0644\u0628\u0627\u0642\u064a.'
        : 'Open /menu to see the rest.',
    )
  return `${header}\n${lines.join('\n')}`.slice(0, 3500)
}
