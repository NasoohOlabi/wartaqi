import { Annotation, StateGraph, START, END } from '@langchain/langgraph'
import { z } from 'zod'
import { neonStudentAttendanceSchema } from '../../shared/neon'
import { cannedReply, fallbackReply } from './agent-copy'

type Lang = 'en' | 'ar'
type Call = (name: string, args: Record<string, unknown>) => Promise<unknown>
export type AgentAccess =
  | { kind: 'guest' }
  | { kind: 'student'; call: Call }
  | { kind: 'staff'; call: Call; tools: ReadonlySet<string> }
export type AgentModel = (
  question: string,
  lang: Lang,
  tools: readonly string[],
) => Promise<unknown>
type Input = { question: string; lang: Lang; access: AgentAccess | (() => Promise<AgentAccess>) }
type Result = {
  text: string
  source: 'canned' | 'tool' | 'fallback'
  tool?: string
  reason?: string
}
type Reason = 'unsupported' | 'unavailable' | 'invalid' | 'denied'
const state = Annotation.Root({ step: Annotation<'route' | 'model' | 'execute' | 'end'> })
const courseSchema = z
  .array(z.object({ id: z.uuid(), name: z.string().max(200), open: z.boolean() }))
  .max(10000)
const studentSchema = z.object({
  total: z.number().int().nonnegative(),
  students: z
    .array(z.object({ name: z.string().max(200), active: z.boolean(), level: z.string().max(200) }))
    .max(50),
})
const sessionSchema = z.object({
  date: z.iso.date(),
  course: z.object({ name: z.string().max(200) }).nullable(),
  session: z
    .array(
      z.object({
        name: z.string().max(200),
        status: z.enum(['present', 'absent', 'excused', 'unmarked']),
      }),
    )
    .max(10000),
})
const plans = z.discriminatedUnion('tool', [
  z.strictObject({ tool: z.literal('list_courses'), args: z.strictObject({}) }),
  z.strictObject({ tool: z.literal('get_my_attendance'), args: z.strictObject({}) }),
  z.strictObject({
    tool: z.literal('find_students'),
    args: z.strictObject({ query: z.string().trim().min(1).max(100) }),
  }),
  z.strictObject({
    tool: z.literal('get_course_attendance_view'),
    args: z.strictObject({ course: z.string().trim().min(1).max(100), date: z.iso.date() }),
  }),
])
const normalize = (value: string) =>
  value.normalize('NFKC').toLocaleLowerCase().replace(/\s+/g, ' ').trim()
const grounded = (part: string, question: string) => normalize(question).includes(normalize(part))
const attendanceIntent =
  /\b(attendance|absent|absence|present|excused)\b|حضور|غياب|غائب|غائبة|حاضر|معذور/i
const ownAttendance =
  /\b(my (?:attendance|absences?)|was i|am i|have i)\b|حضوري|غيابي|كنت حاضر|كنت غايب/i
const studentIntent =
  /\b(student|students|pupil|find|search|look up|register)\b|طالب|طلاب|طالبة|ابحث|بحث/i
const courseIntent = /\b(course|courses|class|classes)\b|دور[ةات]|صفوف|صف/i
const writeIntent =
  /\b(add|create|delete|remove|change|edit|mark|record|save|update|enroll|withdraw|set|correct)\b|أضف|اضف|أنشئ|انشئ|حذف|احذف|عدّل|عدل|غيّر|غير|^سج[ّ]?ل\s|تسجيل|صحح|تعديل/i
const dateOrSpecific =
  /\b\d{4}-\d{2}-\d{2}\b|\b(today|yesterday|tomorrow|week|month|course|class|on|for)\b|اليوم|أمس|امس|غداً|غدا|أسبوع|اسبوع|شهر|دورة|دوره/i
const statuses = {
  en: { present: 'Present', absent: 'Absent', excused: 'Excused', unmarked: 'Unmarked' },
  ar: { present: 'حاضر', absent: 'غائب', excused: 'معذور', unmarked: 'لم يسجل' },
}

export function createAgent(model?: AgentModel) {
  return async (input: Input): Promise<Result> => {
    let result: Result = {
      text: fallbackReply(input.lang, 'unsupported'),
      source: 'fallback',
      reason: 'unsupported',
    }
    const fallback = (reason: Reason): Result => ({
      text: fallbackReply(input.lang, reason),
      source: 'fallback',
      reason,
    })
    const output = (tool: string, text: string): Result => {
      const clean = text.trim()
      const suffix =
        input.lang === 'ar' ? '\nافتح /menu لعرض البقية.' : '\nOpen /menu to see the rest.'
      return {
        text: clean.length > 3500 ? clean.slice(0, 3500 - suffix.length).trimEnd() + suffix : clean,
        source: 'tool',
        tool,
      }
    }
    const question = input.question.trim().slice(0, 1000)
    let access: Exclude<AgentAccess, { kind: 'guest' }> | undefined
    let tools: string[] = []
    let proposal: unknown
    const route = async (): Promise<'model' | 'execute' | 'end'> => {
      const canned = cannedReply(question, input.lang)
      if (canned) {
        result = { text: canned, source: 'canned' }
        return 'end'
      }
      if (!question || writeIntent.test(question)) return 'end'
      if (
        /password|passcode|secret|token|verification code|كلمة المرور|رمز التحقق|\b\d{7,}\b|[\w.+-]+@[\w.-]+\.[a-z]{2,}/i.test(
          question,
        )
      )
        return 'end'
      let resolved: AgentAccess
      try {
        resolved = typeof input.access === 'function' ? await input.access() : input.access
      } catch {
        result = fallback('unavailable')
        return 'end'
      }
      if (resolved.kind === 'guest') {
        result = fallback(
          attendanceIntent.test(question) ||
            studentIntent.test(question) ||
            courseIntent.test(question)
            ? 'denied'
            : 'unsupported',
        )
        return 'end'
      }
      access = resolved
      tools =
        resolved.kind === 'student'
          ? ['get_my_attendance']
          : ['list_courses', 'find_students', 'get_course_attendance_view'].filter((name) =>
              resolved.tools.has(name),
            )
      if (!tools.length) {
        result = fallback('denied')
        return 'end'
      }
      const studentSearch = question.match(
        /^(?:find|search for|look up)\s+(?:student\s+)?(.+?)[?.!]*$/i,
      )
      const sessionSearch = question.match(
        /\battendance for (?:course )?(.+?) on (\d{4}-\d{2}-\d{2})\b/i,
      )
      if (access.kind === 'student' && ownAttendance.test(question)) {
        if (dateOrSpecific.test(question)) {
          result = {
            ...fallback('unsupported'),
            text:
              input.lang === 'ar'
                ? 'افتح /menu ثم حضوري لعرض السجل حسب الدورة أو التاريخ.'
                : 'Open /menu, then My attendance, to check the full record for that course or date.',
          }
          return 'end'
        }
        proposal = { tool: 'get_my_attendance', args: {} }
      } else if (
        access.kind === 'staff' &&
        courseIntent.test(question) &&
        !attendanceIntent.test(question) &&
        !studentIntent.test(question) &&
        /\b(list|show|my|what|which)\b|اعرض|قائمة|دوراتي|ما هي/i.test(question)
      ) {
        proposal = { tool: 'list_courses', args: {} }
      } else if (access.kind === 'staff' && studentSearch && studentIntent.test(question)) {
        proposal = { tool: 'find_students', args: { query: studentSearch[1].trim() } }
      } else if (access.kind === 'staff' && sessionSearch) {
        proposal = {
          tool: 'get_course_attendance_view',
          args: { course: sessionSearch[1].trim(), date: sessionSearch[2] },
        }
      } else {
        return 'model'
      }
      return 'execute'
    }
    const execute = async (): Promise<Result> => {
      if (!access) return fallback('denied')
      const parsed = plans.safeParse(proposal)
      if (!parsed.success) return fallback('invalid')
      const plan = parsed.data
      if (!tools.includes(plan.tool)) return fallback('denied')
      if (
        plan.tool === 'get_my_attendance' &&
        (!ownAttendance.test(question) || dateOrSpecific.test(question))
      )
        return fallback('unsupported')
      if (
        plan.tool === 'list_courses' &&
        (!courseIntent.test(question) ||
          attendanceIntent.test(question) ||
          studentIntent.test(question))
      )
        return fallback('unsupported')
      if (
        plan.tool === 'find_students' &&
        (!studentIntent.test(question) || !grounded(plan.args.query, question))
      )
        return fallback('invalid')
      if (
        plan.tool === 'get_course_attendance_view' &&
        (!attendanceIntent.test(question) ||
          !grounded(plan.args.course, question) ||
          !grounded(plan.args.date, question))
      )
        return fallback('invalid')
      try {
        if (plan.tool === 'get_my_attendance') {
          const data = neonStudentAttendanceSchema.parse(await access.call(plan.tool, {}))
          const counts = { present: 0, absent: 0, excused: 0 }
          for (const item of data.attendance) counts[item.status]++
          const recent = [...data.attendance]
            .sort((a, b) => b.date.localeCompare(a.date))
            .slice(0, 5)
          const heading = input.lang === 'ar' ? 'ملخص حضورك' : 'Your attendance summary'
          const totals = (['present', 'absent', 'excused'] as const)
            .map((key) => `${statuses[input.lang][key]}: ${counts[key]}`)
            .join(' | ')
          return output(
            plan.tool,
            [
              heading,
              totals,
              ...recent.map(
                (item) =>
                  `${item.date}${item.course ? ` — ${item.course}` : ''}: ${statuses[input.lang][item.status]}`,
              ),
            ].join('\n'),
          )
        }
        if (plan.tool === 'list_courses') {
          const courses = courseSchema.parse(await access.call(plan.tool, {}))
          return output(
            plan.tool,
            courses.length
              ? [
                  ...courses
                    .slice(0, 30)
                    .map(
                      (course) =>
                        `${course.name} — ${input.lang === 'ar' ? (course.open ? 'مفتوحة' : 'مغلقة') : course.open ? 'Open' : 'Closed'}`,
                    ),
                  ...(courses.length > 30
                    ? [
                        input.lang === 'ar'
                          ? 'افتح /menu لعرض البقية.'
                          : 'Open /menu to see the rest.',
                      ]
                    : []),
                ].join('\n')
              : input.lang === 'ar'
                ? 'لا توجد دورات متاحة.'
                : 'No courses available.',
          )
        }
        if (plan.tool === 'find_students') {
          const data = studentSchema.parse(
            await access.call(plan.tool, {
              query: plan.args.query,
              status: 'all',
              offset: 0,
              limit: 20,
            }),
          )
          return output(
            plan.tool,
            data.students.length
              ? [
                  `${input.lang === 'ar' ? 'النتائج' : 'Results'}: ${data.total}`,
                  ...data.students.map(
                    (student) =>
                      `${student.name}${student.level ? ` — ${student.level}` : ''} (${input.lang === 'ar' ? (student.active ? 'نشط' : 'غير نشط') : student.active ? 'Active' : 'Inactive'})`,
                  ),
                  ...(data.total > data.students.length
                    ? [
                        input.lang === 'ar'
                          ? 'افتح /menu لعرض البقية.'
                          : 'Open /menu to see the rest.',
                      ]
                    : []),
                ].join('\n')
              : input.lang === 'ar'
                ? 'لم أجد طلاباً بهذا الاسم. جرّب اسماً آخر أو افتح /menu.'
                : 'No students found. Try another name or open /menu.',
          )
        }
        if (access.kind !== 'staff' || !access.tools.has('list_courses')) return fallback('denied')
        const courses = courseSchema
          .parse(await access.call('list_courses', {}))
          .filter((course) => normalize(course.name) === normalize(plan.args.course))
        if (courses.length !== 1)
          return {
            ...fallback('invalid'),
            text:
              input.lang === 'ar'
                ? 'لم أجد دورة مطابقة واحدة. افتح /menu واختر الدورة.'
                : 'Could not find one matching course. Open /menu and select the course.',
          }
        const data = sessionSchema.parse(
          await access.call(plan.tool, {
            courseId: courses[0].id,
            date: plan.args.date,
            view: 'session',
            language: input.lang,
          }),
        )
        if (data.date !== plan.args.date || data.course?.name !== courses[0].name)
          return fallback('invalid')
        return output(
          plan.tool,
          [
            `${data.course.name} — ${data.date}`,
            ...data.session
              .slice(0, 30)
              .map((row) => `${row.name}: ${statuses[input.lang][row.status]}`),
            ...(data.session.length > 30
              ? [input.lang === 'ar' ? 'افتح /menu لعرض البقية.' : 'Open /menu to see the rest.']
              : []),
          ].join('\n'),
        )
      } catch {
        return fallback('unavailable')
      }
    }
    // Graph state holds only routing markers; sensitive data stays in the closure.
    const graph = new StateGraph(state)
      .addNode('route', async () => ({ step: await route() }))
      .addNode('model', async () => {
        if (!model) {
          result = fallback('unavailable')
          return { step: 'end' as const }
        }
        let timer: ReturnType<typeof setTimeout> | undefined
        try {
          proposal = await Promise.race([
            model(question, input.lang, tools),
            new Promise<never>((_, reject) => {
              timer = setTimeout(() => reject(new Error('timeout')), 10_000)
            }),
          ])
          return { step: 'execute' as const }
        } catch {
          result = fallback('unavailable')
          return { step: 'end' as const }
        } finally {
          clearTimeout(timer)
        }
      })
      .addNode('execute', async () => {
        result = await execute()
        return { step: 'end' as const }
      })
      .addEdge(START, 'route')
      .addConditionalEdges('route', ({ step }) =>
        step === 'model' ? 'model' : step === 'execute' ? 'execute' : END,
      )
      .addConditionalEdges('model', ({ step }) => (step === 'execute' ? 'execute' : END))
      .addEdge('execute', END)
      .compile()
    // Refuse to run if tracing is enabled; LangChain can enable it globally from the environment.
    if (
      process.env.LANGCHAIN_TRACING ||
      process.env.LANGCHAIN_TRACING_V2 ||
      process.env.LANGSMITH_TRACING
    )
      return fallback('unavailable')
    try {
      await graph.invoke({ step: 'route' })
    } catch {
      return fallback('unavailable')
    }
    return result
  }
}
