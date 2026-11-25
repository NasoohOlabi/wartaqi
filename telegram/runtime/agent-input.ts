import { z } from 'zod'

const date = z.iso.date()

export function hasPrivateAgentInput(question: string) {
  const text = question
    .replace(/[\u0660-\u0669\u06f0-\u06f9]/g, (digit) => {
      const code = digit.charCodeAt(0)
      return String(code - (code < 0x06f0 ? 0x0660 : 0x06f0))
    })
    .replace(/(?<![+\p{Decimal_Number}-])\b\d{4}-\d{2}-\d{2}\b(?!-\d)/gu, (value) =>
      date.safeParse(value).success ? ' ' : value,
    )
  return /(?:\p{Decimal_Number}[\s()+-]*){8,}|(?:password|passcode|token|secret|كلمة السر|رمز التحقق)\s*(?:[:=]|\bis\b)\s*\S+/iu.test(
    text,
  )
}
