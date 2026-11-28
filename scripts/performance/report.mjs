import { readFileSync } from 'node:fs'

const [file] = process.argv.slice(2)
if (!file) throw new Error('Usage: node scripts/performance/report.mjs PERFORMANCE_JSON')
const result = JSON.parse(readFileSync(file, 'utf8'))
const active = result.activePhase
  ? new Set(
      result.steps
        .slice(result.steps.findIndex(({ label }) => label === result.activePhase))
        .map(({ label }) => label),
    )
  : null
const requests = result.requests.filter((request) =>
  active ? active.has(request.phase) : request.phase !== 'setup',
)
const api = requests.filter(
  ({ path }) => path.startsWith('/api/') || path.startsWith('/socket.io/'),
)
const sum = (rows, key) => rows.reduce((total, row) => total + row[key], 0)
const percentile = (values, percent) => {
  const ordered = [...values].sort((left, right) => left - right)
  return ordered[Math.ceil((ordered.length * percent) / 100) - 1] ?? 0
}
const bytes = (rows) => sum(rows, 'sent') + sum(rows, 'received')
const samples = result.resources
const first = samples[0]
const last = samples.at(-1)
const cpu = (name) => (last[name]?.usage_usec ?? 0) - (first[name]?.usage_usec ?? 0)
const workingSet = (sample) =>
  ['web', 'database'].reduce(
    (total, name) =>
      total + Math.max(0, (sample[name]?.memory_bytes ?? 0) - (sample[name]?.inactive_file ?? 0)),
    0,
  )
const paths = Map.groupBy(requests, ({ method, path }) => method + ' ' + path)

console.log({
  requests: requests.length,
  apiRequests: api.length,
  wireBytes: bytes(requests),
  apiWireBytes: bytes(api),
  incomplete: requests.filter(({ finished }) => !finished).length,
  notModified: requests.filter(({ status }) => status === 304).length,
  httpErrors: requests.filter(({ status }) => status >= 400).length,
  webCpuMicros: cpu('web'),
  dbCpuMicros: cpu('database'),
  combinedWorkingSetP95: percentile(samples.map(workingSet), 95),
  combinedWorkingSetPeak: Math.max(...samples.map(workingSet)),
  webPeak: last.web?.peak_bytes,
  databasePeak: last.database?.peak_bytes,
  successfulStressOps: result.operations?.length,
  stressP95:
    result.operations &&
    percentile(
      result.operations.map(({ ms }) => ms),
      95,
    ),
})
console.table(
  [...paths.entries()]
    .map(([path, rows]) => ({ path, requests: rows.length, bytes: bytes(rows) }))
    .sort((left, right) => right.bytes - left.bytes)
    .slice(0, 20),
)
if (result.steps) console.table(result.steps)
