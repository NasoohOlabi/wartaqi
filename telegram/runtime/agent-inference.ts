import { z } from 'zod'
import type { AgentTool } from './agent-tools'

const callSchema = z.object({
  id: z.string().min(1).max(200),
  type: z.literal('function'),
  function: z.object({ name: z.string().min(1).max(100), arguments: z.string().max(16000) }),
})
const messageSchema = z.discriminatedUnion('role', [
  z.object({ role: z.literal('system'), content: z.string().max(12000) }),
  z.object({ role: z.literal('user'), content: z.string().max(12000) }),
  z.object({
    role: z.literal('assistant'),
    content: z.string().max(4000).nullable(),
    tool_calls: z.array(callSchema).max(1).optional(),
  }),
  z.object({
    role: z.literal('tool'),
    content: z.string().max(12000),
    tool_call_id: z.string().max(200),
  }),
])
export type ProductionMessage = z.infer<typeof messageSchema>
export type ProductionModel = (
  messages: ProductionMessage[],
  tools: readonly AgentTool[],
) => Promise<unknown>
export const productionResponseSchema = z.object({
  role: z.literal('assistant'),
  content: z.string().max(4000).nullable().optional().default(null),
  tool_calls: z.array(callSchema).max(1).optional(),
})

export function createProductionModel(
  config: { url: string; model: string; key: string },
  request: typeof fetch = fetch,
): ProductionModel {
  return async (messages, tools) => {
    const response = await request(config.url, {
      method: 'POST',
      redirect: 'error',
      signal: AbortSignal.timeout(15000),
      headers: { 'content-type': 'application/json', authorization: `Bearer ${config.key}` },
      body: JSON.stringify({
        model: config.model,
        temperature: 0,
        max_tokens: 700,
        messages,
        tools: tools.map((tool) => ({
          type: 'function',
          function: {
            name: tool.name,
            description: tool.description,
            parameters: tool.inputSchema,
          },
        })),
        tool_choice: 'auto',
      }),
    })
    if (!response.ok) throw new Error('Model unavailable')
    const body = z
      .object({ choices: z.array(z.object({ message: productionResponseSchema })).length(1) })
      .parse(await response.json())
    return body.choices[0].message
  }
}
