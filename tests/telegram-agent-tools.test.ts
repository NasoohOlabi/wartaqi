import { describe, expect, test } from 'vitest-runner'
import { z } from 'zod'
import { listenFetch } from '../server/http'
import { Mcp, type McpTool } from '../telegram/transport'
import { payloadSchema } from '../shared/ledger'
import {
  agentCatalog,
  previewAgentWrite,
  toolResultForChat,
  toolResultForModel,
  validateAgentCall,
} from '../telegram/runtime/agent-tools'

const tool = (
  name: string,
  schema: z.ZodType = z.object({ query: z.string().min(1) }).strict(),
): McpTool => ({
  name,
  description: 'test',
  inputSchema: z
    .object({ type: z.literal('object') })
    .catchall(z.unknown())
    .parse(z.toJSONSchema(schema)),
})

describe('agent MCP policy', () => {
  test('intersects discovery and denies privileged and unknown operations', () => {
    expect(
      agentCatalog([
        tool('find_students'),
        tool('reset_account_password'),
        tool('get_notebook'),
        tool('future_tool'),
      ]).map((item) => item.name),
    ).toEqual(['find_students'])
  })
  test('enforces server schema and write event restrictions', () => {
    const [read] = agentCatalog([tool('find_students')])
    expect(validateAgentCall(read, { query: 'Lina' })).toEqual({ query: 'Lina' })
    expect(() => validateAgentCall(read, { query: '' })).toThrow()
    expect(() => validateAgentCall(read, { query: 'x', extra: true })).toThrow()
    const [write] = agentCatalog([
      tool(
        'append_events',
        z.object({ events: z.array(z.object({ payload: z.object({ kind: z.string() }) })) }),
      ),
    ])
    expect(() => validateAgentCall(write, { events: [{ payload: { kind: 'student' } }] })).toThrow()
    expect(() =>
      validateAgentCall(write, { events: [{ payload: { kind: 'attendance' } }] }),
    ).toThrow()
  })
  test('redacts private and unknown fields from model and fallback', () => {
    const input = {
      student: { studentId: 'id', name: 'Demo', phone: 'secret', password: 'secret' },
      notes: [{ text: 'private' }],
      rows: [{ name: 'Demo', text: 'private' }],
    }
    expect(toolResultForModel('get_student_details', input)).toEqual({
      student: { studentId: 'id', name: 'Demo' },
      rows: [{ name: 'Demo' }],
    })
    expect(toolResultForChat('get_student_details', input, 'en')).not.toContain('private')
    expect(toolResultForModel('get_notebook', input)).toBeNull()
  })
  test('shows exact values and resolved targets in write preview', () => {
    const [write] = agentCatalog([
      tool(
        'save_course',
        z.object({ input: z.object({ courseId: z.string(), name: z.string() }) }),
      ),
    ])
    const args = { input: { courseId: 'course-1', name: 'New name' } }
    expect(() => previewAgentWrite(write, args, 'en', {})).toThrow()
    expect(previewAgentWrite(write, args, 'en', { 'course-1': 'Old name' })).toContain(
      'Course: Old name\nName: New name',
    )
    expect(previewAgentWrite(write, args, 'ar', { 'course-1': 'دورة' })).toContain('الدورة: دورة')
  })
  test('accepts real note and reversal payloads while refusing unversioned attendance', () => {
    const id = 'a0d64cf0-18f8-448c-8e9e-71bb66fc9c06'
    const schema = z.object({
      events: z
        .array(z.object({ id: z.uuid(), clientTime: z.iso.datetime(), payload: payloadSchema }))
        .min(1)
        .max(500),
    })
    const [write] = agentCatalog([tool('append_events', schema)])
    const note = {
      events: [
        {
          id,
          clientTime: '2026-11-25T00:00:00.000Z',
          payload: {
            kind: 'note',
            studentId: id,
            date: '2026-11-25',
            tags: ['follow up'],
            text: 'A note',
          },
        },
      ],
    }
    expect(validateAgentCall(write, note)).toEqual(note)
    const preview = previewAgentWrite(write, note, 'en', { [id]: 'Demo Student' })
    expect(preview).toContain('Student: Demo Student')
    expect(preview).toContain('Note: A note')
    expect(preview).not.toContain(id)
    const reversal = {
      events: [
        {
          id,
          clientTime: '2026-11-25T00:00:00.000Z',
          payload: { kind: 'reversal', targetId: id, reason: 'Wrong entry' },
        },
      ],
    }
    expect(
      previewAgentWrite(write, reversal, 'en', { [id]: 'Demo Student, November 25' }),
    ).toContain('Entry to correct: Demo Student, November 25')
  })
  test('collects paged catalog and detects cursor loops', async () => {
    let repeated = false
    const server = await listenFetch(
      async (request) => {
        const body = z
          .object({ params: z.object({ cursor: z.string().optional() }) })
          .parse(await request.json())
        const cursor = body.params.cursor
        return Response.json({
          result: {
            tools: [tool(cursor ? 'list_courses' : 'find_students')],
            ...(cursor && !repeated ? {} : { nextCursor: 'next' }),
          },
        })
      },
      { hostname: '127.0.0.1', port: 0 },
    )
    try {
      const mcp = new Mcp(server.origin, 'token')
      expect([...(await mcp.tools())]).toEqual(['find_students', 'list_courses'])
      repeated = true
      await expect(mcp.catalog()).rejects.toThrow()
    } finally {
      await server.close()
    }
  })
})
