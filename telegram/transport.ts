import { z } from 'zod'

const rpcSchema = z.object({ result: z.unknown().optional(), error: z.unknown().optional() })
const toolSchema = z.object({
  name: z.string().min(1).max(100),
  description: z.string().max(4000).optional().default(''),
  inputSchema: z.object({ type: z.literal('object') }).catchall(z.unknown()),
})
export type McpTool = z.infer<typeof toolSchema>
const toolsSchema = z.object({
  tools: z.array(toolSchema).max(200),
  nextCursor: z.string().min(1).max(1000).optional(),
})
const resultSchema = z
  .object({
    structuredContent: z.unknown().optional(),
    content: z.array(z.unknown()).optional(),
    isError: z.boolean().optional(),
  })
  .passthrough()
const resourceSchema = z.object({
  type: z.literal('resource'),
  resource: z.object({ mimeType: z.string().optional(), blob: z.string().optional() }),
})

export class Mcp {
  constructor(
    private origin: string,
    private token: string,
    private workerSecret?: string,
  ) {}
  private async request(method: string, params: Record<string, unknown> = {}) {
    const response = await fetch(
      `${this.origin}${this.workerSecret ? '/api/telegram/teacher-mcp' : '/mcp'}`,
      {
        method: 'POST',
        signal: AbortSignal.timeout(15_000),
        redirect: 'error',
        headers: {
          authorization: `Bearer ${this.workerSecret ?? this.token}`,
          ...(this.workerSecret ? { 'x-telegram-grant': this.token } : {}),
          'content-type': 'application/json',
          accept: 'application/json, text/event-stream',
        },
        body: JSON.stringify({ jsonrpc: '2.0', id: crypto.randomUUID(), method, params }),
      },
    )
    if (!response.ok) throw new Error(`MCP request failed (${response.status})`)
    const body = rpcSchema.parse(await response.json())
    if (body.error || body.result === undefined) throw new Error('MCP request failed')
    return body.result
  }
  async tools() {
    const names = new Set<string>()
    const seen = new Set<string>()
    let cursor: string | undefined
    for (let page = 0; page < 10; page++) {
      const result = z
        .object({
          tools: z.array(z.object({ name: z.string().min(1).max(100) })).max(200),
          nextCursor: z.string().min(1).max(1000).optional(),
        })
        .parse(await this.request('tools/list', cursor ? { cursor } : {}))
      for (const tool of result.tools) {
        if (names.has(tool.name)) throw new Error('Duplicate MCP tool')
        names.add(tool.name)
      }
      if (names.size > 500) throw new Error('MCP catalog too large')
      if (!result.nextCursor) return names
      if (seen.has(result.nextCursor)) throw new Error('Repeated MCP cursor')
      seen.add(result.nextCursor)
      cursor = result.nextCursor
    }
    throw new Error('MCP catalog exceeds page limit')
  }
  async catalog(): Promise<McpTool[]> {
    const tools: McpTool[] = []
    const seen = new Set<string>()
    let cursor: string | undefined
    for (let page = 0; page < 10; page++) {
      const result = toolsSchema.parse(await this.request('tools/list', cursor ? { cursor } : {}))
      for (const tool of result.tools) {
        if (tools.some((item) => item.name === tool.name)) throw new Error('Duplicate MCP tool')
        tools.push(tool)
      }
      if (tools.length > 500) throw new Error('MCP catalog too large')
      if (!result.nextCursor) return tools
      if (seen.has(result.nextCursor)) throw new Error('Repeated MCP cursor')
      seen.add(result.nextCursor)
      cursor = result.nextCursor
    }
    throw new Error('MCP catalog exceeds page limit')
  }
  async call(name: string, args: Record<string, unknown> = {}) {
    const result = resultSchema.parse(await this.request('tools/call', { name, arguments: args }))
    if (result.isError) throw new Error('MCP tool failed')
    if (result.structuredContent !== undefined) return result.structuredContent
    for (const item of result.content ?? []) {
      const resource = resourceSchema.safeParse(item)
      if (
        resource.success &&
        resource.data.resource.blob &&
        resource.data.resource.mimeType?.includes('spreadsheet')
      )
        return { blob: resource.data.resource.blob }
    }
    const text = z.object({ type: z.literal('text'), text: z.string() })
    const item = result.content
      ?.map((value) => text.safeParse(value))
      .find((value) => value.success)
    if (!item?.success) return result
    try {
      return JSON.parse(item.data.text)
    } catch {
      return item.data.text
    }
  }
}
