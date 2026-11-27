import { useEffect, useState, type ReactNode } from 'react'
import type { NeonTeacher } from '../shared/neon'
import type { CachedAccess } from './offlineAccess'
import { useLanguage } from './useLanguage'
import { useLedger } from './useLedger'

import { openStaffChanges, StaffLedgerContext } from './staffReplicaContext'

export function StaffReplicaProvider({
  neon,
  cached,
  notebookView,
  children,
}: {
  neon: { userId: string; teacher: NeonTeacher }
  cached?: CachedAccess
  notebookView: boolean
  children: ReactNode
}) {
  const [started, setStarted] = useState(notebookView)
  const ledger = useLedger(neon, cached?.offline ?? false, started || notebookView)
  if (!started && (notebookView || ledger.offlinePendingCount > 0 || ledger.pending.length > 0))
    setStarted(true)
  useEffect(() => {
    if (cached?.offline && ledger.loaded && !ledger.offline) cached.reconnect()
  }, [cached, ledger.loaded, ledger.offline])
  return <StaffLedgerContext.Provider value={ledger}>{children}</StaffLedgerContext.Provider>
}

export function StaffPendingNotice({ needsReview }: { needsReview: boolean }) {
  const { language } = useLanguage()
  const ar = language === 'ar'
  return (
    <div
      role="status"
      className="mb-4 flex flex-wrap items-center gap-3 rounded-lg bg-amber-50 p-3 text-amber-900"
    >
      {needsReview
        ? ar
          ? 'توجد تغييرات تحتاج إلى مراجعتك.'
          : 'Some changes need your review.'
        : ar
          ? 'تغييرات محفوظة على الجهاز بانتظار المزامنة.'
          : 'Changes saved on this device are waiting to sync.'}
      <button className="secondary" onClick={openStaffChanges}>
        {needsReview
          ? ar
            ? 'مراجعة التغييرات'
            : 'Review changes'
          : ar
            ? 'تغييراتي'
            : 'My changes'}
      </button>
    </div>
  )
}
