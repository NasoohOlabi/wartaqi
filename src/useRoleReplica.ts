import {
  createContext,
  createElement,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { NeonRequestError, request } from './neon'
import {
  adminBoardReplica,
  parseRoleProjection,
  readRoleReplica,
  replicaScope,
  type RoleReplica,
  writeRoleReplica,
} from './roleReplica'
import { subscribeWorkspaceChanges } from './syncStream'

export function useRoleReplica<T>(definition: RoleReplica<T>, userId: string, accountId?: string) {
  const generation = replicaScope(definition, userId)?.generation
  const [snapshot, setSnapshot] = useState<{
    userId: string
    generation: string | undefined
    data: T
  } | null>(null)
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(true)
  const activeRefresh = useRef<(() => Promise<void>) | null>(null)
  const refresh = useCallback(() => activeRefresh.current?.(), [])

  useEffect(() => {
    let disposed = false
    let confirmed = false
    let flight: Promise<void> | null = null
    let trailing = false
    const stillCurrent = () =>
      !disposed && generation === replicaScope(definition, userId)?.generation
    if (generation)
      void readRoleReplica(definition, userId, generation)
        .then((data) => {
          if (data && !confirmed && stillCurrent()) setSnapshot({ userId, generation, data })
        })
        .catch(() => {})
    async function pull() {
      do {
        trailing = false
        try {
          const value = await request(definition.path, userId)
          if (!stillCurrent()) return
          const id = replicaScope(definition, userId)?.account.id ?? accountId
          if (definition.matches && !id) throw new NeonRequestError(403)
          const data = parseRoleProjection(definition, value, id ?? '')
          confirmed = true
          setSnapshot({ userId, generation, data })
          setError('')
          if (generation)
            void writeRoleReplica(definition, userId, generation, data).catch(() => {})
        } catch (cause) {
          if (!stillCurrent()) return
          if (cause instanceof NeonRequestError && (cause.status === 401 || cause.status === 403)) {
            confirmed = true
            setSnapshot(null)
          }
          setError(cause instanceof Error ? cause.message : new NeonRequestError(500).message)
        } finally {
          if (stillCurrent()) setLoading(false)
        }
      } while (trailing && stillCurrent())
    }
    function schedule() {
      if (flight) {
        trailing = true
        return flight
      }
      flight = pull().finally(() => {
        flight = null
      })
      return flight
    }
    activeRefresh.current = schedule
    const unsubscribe = subscribeWorkspaceChanges(userId, schedule, {
      skipLedger: definition.path === adminBoardReplica.path,
    })
    return () => {
      disposed = true
      activeRefresh.current = null
      unsubscribe()
    }
  }, [accountId, definition, generation, userId])

  const data =
    snapshot?.userId === userId && snapshot.generation === generation ? snapshot.data : null
  return { data, error, loading, refresh }
}

type AdminBoardReplica = ReturnType<
  typeof useRoleReplica<ReturnType<typeof adminBoardReplica.schema.parse>>
>
const AdminReplicaContext = createContext<AdminBoardReplica | null>(null)

export function AdminReplicaProvider({
  userId,
  children,
}: {
  userId: string
  children: ReactNode
}) {
  const board = useRoleReplica(adminBoardReplica, userId)
  return createElement(AdminReplicaContext.Provider, { value: board }, children)
}

export function useAdminBoardReplica() {
  const board = useContext(AdminReplicaContext)
  if (!board) throw new Error('AdminReplicaProvider is required for the admin board.')
  return board
}
