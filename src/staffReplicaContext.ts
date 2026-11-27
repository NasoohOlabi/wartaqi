import { createContext, useContext } from 'react'
import { openChildView } from './queryState'
import type { useLedger } from './useLedger'

export const StaffLedgerContext = createContext<ReturnType<typeof useLedger> | null>(null)

export function useStaffReplica() {
  const ledger = useContext(StaffLedgerContext)
  if (!ledger) throw new Error('StaffReplicaProvider is required for the notebook.')
  return ledger
}

export function openStaffChanges() {
  openChildView({ view: 'changes' })
}
