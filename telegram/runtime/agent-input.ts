import { z } from 'zod'

const date = z.iso.date()
const datePattern = /(?<![+\p{Decimal_Number}-])\b\d{4}-\d{2}-\d{2}\b(?!-\d)/gu
const normalizeDigits = (text: string) =>
  text.replace(/[\u0660-\u0669\u06f0-\u06f9]/g, (digit) => {
    const code = digit.charCodeAt(0)
    return String(code - (code < 0x06f0 ? 0x0660 : 0x06f0))
  })

export function explicitAgentDates(question: string) {
  return [...new Set(normalizeDigits(question).match(datePattern) ?? [])].filter(
    (value) => date.safeParse(value).success,
  )
}

export function hasPrivateAgentInput(question: string) {
  if (
    /(?:password|passcode|token|secret|كلمة (?:السر|المرور)|رمز التحقق)\s*(?:[:=]|\bis\b|ه[وي](?=\s))\s*\S+/iu.test(
      question,
    )
  )
    return true
  const text = normalizeDigits(question).replace(datePattern, (value) =>
    date.safeParse(value).success ? ' ' : value,
  )
  return /(?:\p{Decimal_Number}[\s()+-]*){8,}/u.test(text)
}
