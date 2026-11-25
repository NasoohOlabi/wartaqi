import { internationalPhone, phoneNumber } from './phoneFormat'
import { useLanguage } from './useLanguage'

export function PhoneNumber({ value }: { value: string }) {
  const { display, href } = phoneNumber(value)
  const { language } = useLanguage()
  const international = internationalPhone(value)
  return (
    <span className="inline-flex max-w-full flex-wrap items-center gap-x-2">
      {href ? (
        <a
          href={href}
          dir="ltr"
          className="items-center"
          style={{
            display: 'inline-flex',
            minHeight: 44,
            minWidth: 44,
            maxWidth: '100%',
            overflowWrap: 'anywhere',
            wordBreak: 'break-all',
            textDecoration: 'underline',
            unicodeBidi: 'isolate',
          }}
        >
          {display}
        </a>
      ) : (
        <bdi dir="ltr">{display}</bdi>
      )}
      {international && (
        <a
          href={`https://wa.me/${international.slice(1)}`}
          target="_blank"
          rel="noreferrer"
          aria-label={language === 'ar' ? `محادثة واتساب: ${display}` : `WhatsApp chat: ${display}`}
          title={language === 'ar' ? 'محادثة واتساب' : 'WhatsApp chat'}
          className="inline-flex min-h-11 min-w-11 items-center justify-center underline"
        >
          {language === 'ar' ? 'واتساب' : 'WhatsApp'}
        </a>
      )}
    </span>
  )
}
