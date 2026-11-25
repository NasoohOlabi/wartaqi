import { expect, test } from 'bun:test'
import { randomBytes } from 'node:crypto'
import { Pool } from 'pg'
import { z } from 'zod'
import { createWorker } from '../telegram/runtime'
import { revokeTelegramGrants } from '../shared/telegram-grants'
import { createIdentity } from './integration-identity'
import { startIntegrationServer } from './integration-server'

const database = process.env.TEST_RAILWAY_DATABASE_URL
const modelOrigin = process.env.TEST_LM_STUDIO_ORIGIN
const live = database && modelOrigin ? test : test.skip

live(
  'real worker, disposable database, MCP and LM Studio with simulated Telegram delivery',
  async () => {
    if (!database || !modelOrigin) throw new Error('Disposable database and local model required')
    const dbUrl = new URL(database)
    const modelUrl = new URL(modelOrigin)
    if (
      !['127.0.0.1', 'localhost'].includes(dbUrl.hostname) ||
      !['127.0.0.1', 'localhost'].includes(modelUrl.hostname) ||
      modelUrl.protocol !== 'http:'
    )
      throw new Error('Integration test only accepts loopback services')
    const secret = randomBytes(32).toString('hex')
    const pool = new Pool({ connectionString: database })
    const student = crypto.randomUUID()
    const teacher = crypto.randomUUID()
    const course = crypto.randomUUID()
    const enrollment = crypto.randomUUID()
    const initialEvent = crypto.randomUUID()
    const phone = `+1202${String(Math.floor(Math.random() * 10000000)).padStart(7, '0')}`
    let user = ''
    let server: ReturnType<typeof Bun.spawn> | undefined
    let worker: ReturnType<typeof createWorker> | undefined
    const original = globalThis.fetch
    try {
      const previous = process.env.TELEGRAM_WORKER_SECRET
      process.env.TELEGRAM_WORKER_SECRET = secret
      const started = await startIntegrationServer(database).finally(() => {
        if (previous === undefined) delete process.env.TELEGRAM_WORKER_SECRET
        else process.env.TELEGRAM_WORKER_SECRET = previous
      })
      server = started.child
      user = await createIdentity(pool, 'Synthetic teacher', randomBytes(32).toString('hex'))
      await pool.query(
        "INSERT INTO teachers (id, auth_user_id, name, access_mode) VALUES ($1, $2, 'Synthetic teacher', 'read_write')",
        [teacher, user],
      )
      await pool.query("INSERT INTO students (id, teacher_id, name) VALUES ($1, $2, 'Demo Lina')", [
        student,
        teacher,
      ])
      await pool.query(
        "INSERT INTO app_accounts (auth_user_id, role, teacher_id, verified_phone) VALUES ($1, 'teacher', $2, $3)",
        [user, teacher, phone],
      )
      await pool.query("INSERT INTO courses (id, name) VALUES ($1, 'Demo Recitation')", [course])
      await pool.query(
        'INSERT INTO course_enrollments (id, course_id, student_id) VALUES ($1, $2, $3)',
        [enrollment, course, student],
      )
      await pool.query(
        "INSERT INTO events (id, teacher_id, student_id, enrollment_id, actor, client_time, payload) VALUES ($1, $2, $3, $4, 'Synthetic teacher', now(), $5)",
        [
          initialEvent,
          teacher,
          student,
          enrollment,
          {
            kind: 'attendance',
            studentId: student,
            enrollmentId: enrollment,
            date: '2026-11-20',
            status: 'present',
          },
        ],
      )
      const login = await original(`${started.origin}/api/telegram/student`, {
        method: 'POST',
        headers: { authorization: `Bearer ${secret}`, 'content-type': 'application/json' },
        body: JSON.stringify({ action: 'login-phone', phone }),
      })
      expect(login.status).toBe(200)
      const grant = z
        .object({ token: z.string(), role: z.literal('teacher') })
        .parse(await login.json()).token
      let modelRequests = 0
      const toolCalls: string[] = []
      const messages: { text: string; id: number; confirm?: string }[] = []
      globalThis.fetch = Object.assign(
        async (input: string | URL | Request, init?: RequestInit) => {
          const url = new URL(input instanceof Request ? input.url : input)
          const body = z.record(z.string(), z.unknown()).parse(JSON.parse(String(init?.body)))
          if (url.origin === 'https://api.telegram.org') {
            if (url.pathname.endsWith('/sendMessage')) {
              const markup = z
                .object({
                  inline_keyboard: z.array(z.array(z.object({ callback_data: z.string() }))),
                })
                .optional()
                .parse(body.reply_markup)
              messages.push({
                text: z.string().parse(body.text),
                id: messages.length + 1,
                confirm: markup?.inline_keyboard[0]?.[0]?.callback_data,
              })
              return Response.json({ ok: true, result: { message_id: messages.length } })
            }
            return Response.json({ ok: true, result: true })
          }
          if (url.origin === 'https://model.example') {
            modelRequests++
            return original(new URL('/v1/chat/completions', modelUrl), init)
          }
          if (url.origin === 'https://app.example') {
            if (body.method === 'tools/call')
              toolCalls.push(z.object({ name: z.string() }).parse(body.params).name)
            return original(new URL(url.pathname + url.search, started.origin), init)
          }
          throw new Error('Unexpected outbound request')
        },
        { preconnect: original.preconnect },
      )
      worker = createWorker({
        TELEGRAM_BOT_TOKEN: 'synthetic',
        WARTAQI_ORIGIN: 'https://app.example',
        TELEGRAM_PUBLIC_ORIGIN: 'https://bot.example',
        TELEGRAM_DB_PATH: ':memory:',
        TELEGRAM_STORAGE_KEY: randomBytes(32).toString('base64'),
        TELEGRAM_WORKER_SECRET: secret,
        TELEGRAM_LANGGRAPH_ENABLED: '1',
        LM_STUDIO_BASE_URL: 'https://model.example/v1',
        LM_STUDIO_MODEL: 'prism-ml/bonsai-27b',
        LM_STUDIO_API_KEY: 'synthetic',
      })
      worker.store.set('teacher:1', grant)
      const send = (id: number, text: string) =>
        worker?.process({
          update_id: id,
          message: { message_id: id, chat: { id: 1, type: 'private' }, from: { id: 1 }, text },
        })
      await send(1, 'Who was present in Demo Recitation on 2026-11-20?')
      const first = { calls: [...toolCalls], answer: messages.at(-1)?.text ?? '' }
      await send(2, 'And what about 2026-11-21?')
      const second = {
        calls: toolCalls.slice(first.calls.length),
        answer: messages.at(-1)?.text ?? '',
      }
      const readModelRequests = modelRequests
      await send(3, 'Mark Demo Lina absent in Demo Recitation on 2026-11-21.')
      const approval = messages.at(-1)
      expect(approval?.text).toContain('Confirm')
      expect(approval?.text).toContain('Demo Lina')
      expect(approval?.text).toContain('Demo Recitation')
      expect(modelRequests).toBe(readModelRequests)
      expect(toolCalls).not.toContain('sync_attendance')
      const callback = z.string().parse(approval?.confirm)
      const click = (id: number) =>
        worker?.process({
          update_id: id,
          callback_query: {
            id: String(id),
            from: { id: 1 },
            data: callback,
            message: { message_id: approval?.id, chat: { id: 1, type: 'private' } },
          },
        })
      await click(4)
      expect(messages.at(-1)?.text).toContain('Request submitted')
      const rows = await pool.query(
        'SELECT payload FROM events WHERE student_id = $1 AND payload->>$2 = $3',
        [student, 'date', '2026-11-21'],
      )
      expect(rows.rowCount).toBe(1)
      expect(rows.rows[0].payload.status).toBe('absent')
      await click(5)
      expect(
        (
          await pool.query('SELECT id FROM events WHERE student_id = $1 AND payload->>$2 = $3', [
            student,
            'date',
            '2026-11-21',
          ])
        ).rowCount,
      ).toBe(1)
      expect(toolCalls).toContain('sync_attendance')
      console.info(
        JSON.stringify({
          evidence: 'live_model_reads_and_deterministic_confirmed_write',
          modelRequests,
          first,
          second,
          writeTools: toolCalls.slice(first.calls.length + second.calls.length),
          writeRows: 1,
          replayRows: 1,
        }),
      )
      expect(modelRequests).toBeGreaterThan(0)
      expect(first.calls).toContain('list_courses')
      expect(first.calls).toContain('get_course_attendance_view')
      expect(first.answer).toContain('Demo Lina: Present')
      expect(second.calls).toContain('get_course_attendance_view')
      expect(second.answer).toContain('2026-11-21')
      expect(second.answer).toContain('Demo Lina: Unmarked')
      expect(second.answer.toLowerCase()).not.toContain('was present')
      await send(6, 'مين كان غايب بدورة Demo Recitation يوم 2026-11-21؟')
      const arabic = messages.at(-1)?.text ?? ''
      expect(arabic).toContain('Demo Recitation — 2026-11-21')
      expect(arabic).toContain('Demo Lina: غائب')
      console.info(JSON.stringify({ evidence: 'arabic_read_back', answer: arabic }))
      const question = process.env.TEST_TELEGRAM_AGENT_QUESTION?.trim()
      if (question) {
        const previousReplies = messages.length
        await send(7, question)
        expect(messages.length).toBeGreaterThan(previousReplies)
        console.info(
          JSON.stringify({ evidence: 'extra_synthetic_turn', answer: messages.at(-1)?.text }),
        )
      }
    } finally {
      globalThis.fetch = original
      worker?.store.close()
      server?.kill()
      if (user) {
        const db = await pool.connect()
        try {
          await revokeTelegramGrants(db, user)
        } finally {
          db.release()
        }
        await pool.query('DELETE FROM app_accounts WHERE auth_user_id = $1', [user])
      }
      await pool.query('DELETE FROM events WHERE student_id = $1', [student])
      await pool.query('DELETE FROM course_enrollments WHERE id = $1', [enrollment])
      await pool.query('DELETE FROM courses WHERE id = $1', [course])
      await pool.query('DELETE FROM students WHERE id = $1', [student])
      await pool.query('DELETE FROM teachers WHERE id = $1', [teacher])
      if (user) await pool.query('DELETE FROM neon_auth."user" WHERE id = $1', [user])
      await pool.end()
    }
  },
  180000,
)
