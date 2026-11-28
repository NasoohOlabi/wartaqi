import { describe, expect, test } from 'vitest-runner'
import { z } from 'zod'
import { createProductionAgent, agentContextSchema } from '../telegram/runtime/agent-graph'
import { hasPrivateAgentInput } from '../telegram/runtime/agent-input'
import type { AgentTool } from '../telegram/runtime/agent-tools'
import type { ProductionMessage, ProductionModel } from '../telegram/runtime/agent-inference'

const courseId = 'b9fb79f4-2de2-4f72-a2d6-0ec9de6b8c39'
const studentId = '6a4970ac-f751-4928-b6a1-384166982217'
const enrollmentId = '62f0de02-172e-47cb-82e5-b12358fe71b9'
const tools: AgentTool[] = [
  {
    name: 'list_courses',
    description: '',
    kind: 'read',
    inputSchema: { type: 'object', additionalProperties: false },
  },
  {
    name: 'get_course_attendance_view',
    description: '',
    kind: 'read',
    inputSchema: {
      type: 'object',
      properties: { courseId: { type: 'string' } },
      required: ['courseId'],
      additionalProperties: false,
    },
  },
  {
    name: 'sync_attendance',
    description: '',
    kind: 'write',
    inputSchema: {
      type: 'object',
      properties: { records: { type: 'array' } },
      required: ['records'],
      additionalProperties: false,
    },
  },
]
const callMessage = (name: string, args: object) => ({
  role: 'assistant',
  content: null,
  tool_calls: [
    { id: `call-${name}`, type: 'function', function: { name, arguments: JSON.stringify(args) } },
  ],
})

describe('production agent', () => {
  test('uses multiple discovered tools, minimizes data, and carries follow-up context', async () => {
    const calls: string[] = []
    const seen: ProductionMessage[][] = []
    let step = 0
    const model: ProductionModel = async (messages) => {
      seen.push(structuredClone(messages))
      step++
      if (step === 1) return callMessage('list_courses', {})
      if (step === 2) return callMessage('get_course_attendance_view', { courseId })
      return { role: 'assistant', content: 'Maya was absent.' }
    }
    const run = createProductionAgent(model)
    const first = await run({
      question: 'Who was absent on 2026-11-23?',
      lang: 'en',
      tools,
      call: async (name) => {
        calls.push(name)
        return name === 'list_courses'
          ? [{ id: courseId, name: 'Demo', phone: '+963999999999', secret: 'hidden' }]
          : {
              session: [
                { studentId, enrollmentId, name: 'Maya', status: 'absent', note: 'private' },
              ],
            }
      },
    })
    expect(first.kind).toBe('answer')
    expect(calls).toEqual(['list_courses', 'get_course_attendance_view'])
    expect(JSON.stringify(seen)).not.toContain('hidden')
    expect(JSON.stringify(seen)).not.toContain('private')
    expect(
      seen[1].some(
        (message) => message.role === 'tool' && message.tool_call_id === 'call-list_courses',
      ),
    ).toBe(true)
    expect(agentContextSchema.safeParse(first.context).success).toBe(true)
    await run({
      question: 'What about her?',
      lang: 'en',
      tools,
      context: first.context,
      call: async () => ({}),
    })
    expect(JSON.stringify(seen.at(-1))).toContain('Maya')
  })

  test('previews an exact stable write and never executes it', async () => {
    let step = 0
    const model: ProductionModel = async () =>
      ++step === 1
        ? callMessage('get_course_attendance_view', { courseId })
        : callMessage('sync_attendance', {
            records: [
              {
                event: {
                  id: 'replace',
                  clientTime: 'replace',
                  payload: {
                    kind: 'attendance',
                    studentId,
                    enrollmentId,
                    date: '2026-11-23',
                    status: 'present',
                  },
                },
                expectedEventId: null,
              },
            ],
          })
    const calls: string[] = []
    const result = await createProductionAgent(model)({
      question: 'Mark Maya present on 2026-11-23',
      lang: 'en',
      tools,
      call: async (name) => {
        calls.push(name)
        return {
          course: { id: courseId, name: 'Demo Recitation', open: true },
          date: '2026-11-23',
          session: [{ studentId, enrollmentId, name: 'Maya', status: 'unmarked', eventId: null }],
        }
      },
    })
    expect(calls).toEqual(['get_course_attendance_view'])
    expect(result.kind).toBe('approval')
    if (result.kind !== 'approval') throw new Error('Expected approval')
    expect(result.text).toContain('Maya')
    expect(result.text).toContain('2026-11-23')
    const records = result.args.records
    expect(Array.isArray(records)).toBe(true)
    expect(JSON.stringify(records)).not.toContain('replace')
    const event = z
      .array(z.object({ event: z.object({ id: z.uuid(), clientTime: z.iso.datetime() }) }))
      .parse(records)[0].event
    expect(event.id).not.toBe(studentId)
    expect(JSON.stringify(records)).toContain('expectedEventId')
  })

  test('falls back on unknown, malformed, empty, and unavailable model outputs', async () => {
    for (const output of [
      callMessage('not_authorized', {}),
      { role: 'assistant', content: '' },
      { role: 'bad' },
    ]) {
      const result = await createProductionAgent(async () => output)({
        question: 'What happened?',
        lang: 'en',
        tools,
        call: async () => {
          throw new Error('must not call')
        },
      })
      expect(result.kind).toBe('answer')
      if (result.kind === 'answer') expect(result.source).toBe('fallback')
    }
    const result = await createProductionAgent(async () => {
      throw new Error('offline')
    })({ question: 'What happened?', lang: 'ar', tools, call: async () => ({}) })
    expect(result.kind).toBe('answer')
    if (result.kind === 'answer') expect(result.source).toBe('fallback')
  })
  test('uses verified read result when the model fails without repeating a read', async () => {
    let step = 0
    let calls = 0
    const run = createProductionAgent(async () => {
      if (++step === 1) return callMessage('list_courses', {})
      throw new Error('offline')
    })
    const result = await run({
      question: 'Show the available courses',
      lang: 'en',
      tools,
      call: async () => {
        calls++
        return [{ id: courseId, name: 'Demo' }]
      },
    })
    expect(result.kind).toBe('answer')
    expect(result.text).toContain('Demo')
    expect(calls).toBe(1)
  })

  test('does not report an unapproved write as saved', async () => {
    const run = createProductionAgent(async () => ({
      role: 'assistant',
      content: 'Saved successfully.',
    }))
    const result = await run({
      question: 'Mark Maya present',
      lang: 'en',
      tools,
      call: async () => ({}),
    })
    expect(result.text).toContain('No change has been saved')
  })
  test('rejects stale model answers after access is withdrawn', async () => {
    const old = await createProductionAgent(async (messages) =>
      messages.some((message) => message.role === 'tool')
        ? { role: 'assistant', content: 'Maya was absent.' }
        : callMessage('get_course_attendance_view', { courseId }),
    )({
      question: 'Show Demo attendance',
      lang: 'en',
      tools,
      call: async () => ({
        session: [{ studentId, enrollmentId, name: 'Maya', status: 'absent' }],
      }),
    })
    expect(JSON.stringify(old.context)).not.toContain('absent')
    expect(JSON.stringify(old.context)).not.toContain('Maya was absent')
    let attempts = 0
    const result = await createProductionAgent(async () => {
      attempts++
      return { role: 'assistant', content: 'Maya was absent.' }
    })({
      question: 'What about her now?',
      lang: 'en',
      context: old.context,
      tools,
      call: async () => {
        throw new Error('access revoked')
      },
    })
    expect(attempts).toBe(2)
    expect(result.text).not.toContain('Maya was absent')
    expect(result.kind === 'answer' && result.source).toBe('fallback')
  })

  test('repairs stale text by reading current records in English and Arabic', async () => {
    for (const lang of ['en', 'ar'] as const) {
      let attempts = 0
      let reads = 0
      const result = await createProductionAgent(async () => {
        attempts++
        if (attempts === 1) return { role: 'assistant', content: 'Old result' }
        if (attempts === 2) return callMessage('get_course_attendance_view', { courseId })
        return { role: 'assistant', content: 'Current result' }
      })({
        question:
          lang === 'ar'
            ? '\u0627\u0639\u0631\u0636 \u062d\u0636\u0648\u0631 Demo'
            : 'Show attendance for Demo',
        lang,
        tools,
        context: {
          turns: [{ question: 'Show attendance for Demo' }],
          labels: { [courseId]: 'Demo' },
        },
        call: async () => {
          reads++
          return { session: [{ name: 'Maya', status: 'present' }] }
        },
      })
      expect(reads).toBe(1)
      expect(result.text).toBe('Current result')
      expect(JSON.stringify(result.context)).not.toContain('Current result')
      expect(JSON.stringify(result.context)).not.toContain('present')
    }
  })
})

const attendanceTools: AgentTool[] = tools.map((tool) =>
  tool.name === 'get_course_attendance_view'
    ? {
        ...tool,
        inputSchema: {
          type: 'object',
          properties: {
            courseId: { type: 'string' },
            date: { type: 'string' },
            view: { type: 'string' },
            language: { type: 'string' },
          },
          required: ['courseId', 'date', 'view', 'language'],
          additionalProperties: false,
        },
      }
    : tool,
)
const eventId = '7ca0c708-cdcb-4b31-8931-70078a3de5b2'
const course = { id: courseId, name: 'Demo Recitation', open: true }
const row = { studentId, enrollmentId, name: 'Demo Lina', status: 'unmarked', eventId: null }

function attendanceCall(
  options: { courses?: object[]; session?: object[]; open?: boolean; date?: string } = {},
) {
  const calls: string[] = []
  return {
    calls,
    call: async (name: string) => {
      calls.push(name)
      if (name === 'list_courses')
        return options.courses ?? [{ ...course, open: options.open ?? true }]
      if (name === 'get_course_attendance_view')
        return { course, date: options.date ?? '2026-11-20', session: options.session ?? [row] }
      throw new Error('write must not be called')
    },
  }
}

describe('attendance planning', () => {
  test('allows dates without exempting date-like passwords or phone numbers', () => {
    expect(hasPrivateAgentInput('Attendance on 2026-11-21')).toBe(false)
    expect(hasPrivateAgentInput('My password is 2026-11-21')).toBe(true)
    expect(hasPrivateAgentInput('كلمة المرور هي ٢٠٢٦-١١-٢١')).toBe(true)
    expect(hasPrivateAgentInput('The contact is +2026-11-21')).toBe(true)
  })

  test('uses the date typed in the current question rather than the old model date', async () => {
    let step = 0
    const result = await createProductionAgent(async () =>
      ++step === 1
        ? callMessage('get_course_attendance_view', {
            courseId,
            date: '2026-11-20',
            view: 'session',
            language: 'ar',
          })
        : { role: 'assistant', content: 'Show the attendance.' },
    )({
      question: 'مين كان غايب بدورة Demo Recitation يوم ٢٠٢٦-١١-٢١؟',
      lang: 'ar',
      tools: attendanceTools,
      call: async (_name, args) => {
        expect(args.date).toBe('2026-11-21')
        return { course, date: args.date, session: [{ ...row, status: 'absent', eventId }] }
      },
    })
    expect(result.text).toContain('Demo Recitation — 2026-11-21')
    expect(result.text).toContain('Demo Lina: غائب')
  })

  test('prepares an exact attendance change in English and Arabic without writing', async () => {
    for (const question of [
      'Mark Demo Lina absent in Demo Recitation on 2026-11-20',
      '\u0633\u062c\u0644 Demo Lina \u063a\u0627\u0626\u0628 \u0641\u064a Demo Recitation \u0628\u062a\u0627\u0631\u064a\u062e 2026-11-20',
    ]) {
      const io = attendanceCall()
      const result = await createProductionAgent()({
        question,
        lang: question.startsWith('Mark') ? 'en' : 'ar',
        tools: attendanceTools,
        call: io.call,
      })
      expect(result.kind).toBe('approval')
      if (result.kind !== 'approval') throw new Error(result.text)
      expect(io.calls).toEqual(['list_courses', 'get_course_attendance_view'])
      expect(result.text).toContain('Demo Lina')
      expect(result.text).toContain('Demo Recitation')
      const record = z
        .object({
          records: z
            .array(
              z.object({
                event: z.object({
                  id: z.uuid(),
                  clientTime: z.iso.datetime(),
                  payload: z.object({
                    studentId: z.uuid(),
                    enrollmentId: z.uuid(),
                    date: z.iso.date(),
                    status: z.literal('absent'),
                  }),
                }),
                expectedEventId: z.null(),
              }),
            )
            .length(1),
        })
        .parse(result.args).records[0]
      expect(record.event.payload.studentId).toBe(studentId)
      expect(record.event.payload.enrollmentId).toBe(enrollmentId)
    }
  })
  test('retains the current event id and rejects ambiguous, closed, missing and out-of-scope records', async () => {
    const marked = attendanceCall({ session: [{ ...row, status: 'present', eventId }] })
    const ok = await createProductionAgent()({
      question: 'Mark Demo Lina absent in Demo Recitation on 2026-11-20',
      lang: 'en',
      tools: attendanceTools,
      call: marked.call,
    })
    expect(ok.kind).toBe('approval')
    if (ok.kind === 'approval') expect(JSON.stringify(ok.args)).toContain(eventId)
    for (const options of [
      { courses: [course, course] },
      { open: false },
      { session: [row, row] },
      { session: [] },
      { session: [{ ...row, status: 'present', eventId: null }] },
      { date: '2026-11-19' },
    ]) {
      const io = attendanceCall(options)
      const result = await createProductionAgent()({
        question: 'Mark Demo Lina absent in Demo Recitation on 2026-11-20',
        lang: 'en',
        tools: attendanceTools,
        call: io.call,
      })
      expect(result.kind).toBe('answer')
      expect(result.text).toContain('No change saved')
      expect(io.calls).not.toContain('sync_attendance')
    }
    const missing = attendanceCall()
    const result = await createProductionAgent()({
      question: 'Mark Demo Lina absent in Demo Recitation',
      lang: 'en',
      tools: attendanceTools,
      call: missing.call,
    })
    expect(result.text).toContain('YYYY-MM-DD')
    expect(missing.calls).toEqual([])
    const denied = attendanceCall()
    const noWrite = await createProductionAgent()({
      question: 'Mark Demo Lina absent in Demo Recitation on 2026-11-20',
      lang: 'en',
      tools: attendanceTools.filter((tool) => tool.kind === 'read'),
      call: denied.call,
    })
    expect(noWrite.kind).toBe('answer')
    expect(denied.calls).toEqual([])
  })
  test('renders current unmarked status instead of invented model prose', async () => {
    for (const lang of ['en', 'ar'] as const) {
      let count = 0
      const result = await createProductionAgent(async () =>
        ++count === 1
          ? callMessage('get_course_attendance_view', { courseId })
          : { role: 'assistant', content: 'Demo Lina is present and unmarked.' },
      )({
        question: 'Show attendance',
        lang,
        tools,
        call: async () => ({ course, date: '2026-11-20', session: [row] }),
      })
      expect(result.text).toContain(
        lang === 'en'
          ? 'Demo Lina: Unmarked'
          : 'Demo Lina: \u0644\u0645 \u064a\u064f\u0633\u062c\u0644',
      )
      expect(result.text).not.toContain('present and unmarked')
    }
  })
})
