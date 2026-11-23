import { z } from 'zod'

export const telegramConversationIngestPath = '/api/telegram/conversations'
export const telegramConversationsPath = '/api/diagnostics/telegram/conversations'
export const telegramConversationTurnsPath = '/api/diagnostics/telegram/turns'

export const telegramConversationIdSchema = z.string().regex(/^[a-f0-9]{64}$/)
export const telegramConversationCursorSchema = z
  .string()
  .regex(/^[1-9][0-9]{0,18}$/)
  .refine((value) => value.length < 19 || value <= '9223372036854775807')

const outcome = z.union([
  z.enum([
    'ok',
    'unconfigured',
    'malformed',
    'tool_request',
    'output_limit',
    'incomplete',
    'empty',
    'link_output',
    'timeout',
    'network',
  ]),
  z.string().regex(/^http_[1-5][0-9]{2}$/),
])

export const telegramConversationTurnSchema = z.strictObject({
  trace: z.string().regex(/^[A-Za-z0-9_-]{12}$/),
  conversationId: telegramConversationIdSchema,
  question: z.string().min(1).max(1000),
  reply: z.string().min(1).max(3000),
  delivery: z.enum(['sent', 'failed']),
  outcome,
  promptVersion: z.string().regex(/^wartaq-help-v[0-9]{1,8}$/),
  model: z.string().regex(/^[\w./:-]{1,100}$/),
  elapsedMs: z.number().int().nonnegative().max(600_000),
})

export const telegramConversationsSchema = z.strictObject({
  items: z.array(
    z.strictObject({
      conversationId: telegramConversationIdSchema,
      lastSeenAt: z.iso.datetime(),
      turnCount: z.number().int().positive(),
    }),
  ),
  nextCursor: telegramConversationCursorSchema.nullable(),
})

export const telegramConversationTurnsSchema = z.strictObject({
  items: z.array(
    telegramConversationTurnSchema.omit({ conversationId: true }).extend({
      receivedAt: z.iso.datetime(),
    }),
  ),
  nextCursor: telegramConversationCursorSchema.nullable(),
})

export function redactTelegramTranscript(text: string, limit: number) {
  return text
    .replace(/(?:https?:\/\/|www\.)[^\s<>()]+/giu, '[redacted]')
    .replace(/[\w.!#$%&'*+/=?^_`{|}~-]+@[\w.-]+\.[a-z]{2,}/giu, '[redacted]')
    .replace(/\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b/g, '[redacted]')
    .replace(/\+?(?:\p{Decimal_Number}[ ()+-]*){6,}\p{Decimal_Number}/gu, '[redacted]')
    .replace(
      /((?:\b(?:password|passcode|(?:access[_ -]?)?token|secret|(?:verification )?code)\b|كلمة (?:المرور|السر)|رمز التحقق)\s*(?:[:=：]|\bis\b|هو)\s*)[^\s,;]+/giu,
      '$1[redacted]',
    )
    .slice(0, limit)
    .replace(/[\0\uD800-\uDFFF]/gu, '\uFFFD')
}
