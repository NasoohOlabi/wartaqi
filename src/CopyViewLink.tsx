import { Check, Link2, X } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import { currentViewLink } from './queryState'
import { useLanguage } from './useLanguage'
import './copyViewLink.css'

type CopyState = { kind: 'idle' } | { kind: 'copied' } | { kind: 'manual'; href: string }

function writeClipboard(value: string) {
  const write = navigator.clipboard?.writeText(value)
  if (!write) return Promise.reject(new Error('clipboard-unavailable'))
  return new Promise<void>((resolve, reject) => {
    const timer = window.setTimeout(() => reject(new Error('clipboard-timeout')), 1500)
    write.then(
      () => {
        window.clearTimeout(timer)
        resolve()
      },
      (error: unknown) => {
        window.clearTimeout(timer)
        reject(error instanceof Error ? error : new Error('clipboard-failed'))
      },
    )
  })
}

export function CopyViewLink() {
  const { t } = useLanguage()
  const [state, setState] = useState<CopyState>({ kind: 'idle' })
  const writing = useRef(false)
  const statusId = useId()
  const field = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (state.kind !== 'copied') return
    const timer = window.setTimeout(() => setState({ kind: 'idle' }), 2000)
    return () => window.clearTimeout(timer)
  }, [state])

  useEffect(() => {
    if (state.kind !== 'manual') return
    field.current?.focus()
    field.current?.select()
  }, [state])

  async function copy() {
    if (writing.current) return
    const href = currentViewLink()
    writing.current = true
    try {
      await writeClipboard(href)
      setState({ kind: 'copied' })
    } catch {
      setState({ kind: 'manual', href })
    } finally {
      writing.current = false
    }
  }

  const copied = state.kind === 'copied'
  return (
    <div className="copy-view-link">
      {state.kind === 'manual' && (
        <div className="copy-view-fallback">
          <div className="copy-view-fallback-heading">
            <p id={statusId}>{t('Select the link and copy it.')}</p>
            <button
              type="button"
              className="icon-button"
              aria-label={t('Close')}
              onClick={() => setState({ kind: 'idle' })}
            >
              <X size={18} strokeWidth={1.8} aria-hidden="true" />
            </button>
          </div>
          <input ref={field} readOnly value={state.href} aria-labelledby={statusId} />
        </div>
      )}
      <button
        type="button"
        className={copied ? 'is-copied' : undefined}
        aria-label={copied ? t('Link copied') : t('Copy link')}
        onClick={() => void copy()}
      >
        {copied ? (
          <Check size={16} strokeWidth={2} aria-hidden="true" />
        ) : (
          <Link2 size={16} strokeWidth={1.8} aria-hidden="true" />
        )}
      </button>
      <p className="copy-view-status" aria-live="polite" aria-atomic="true">
        {copied ? t('Link copied') : ''}
      </p>
    </div>
  )
}
