import { readFile, writeFile } from 'node:fs/promises'
import { performance } from 'node:perf_hooks'
import { startMeter } from './meter.mjs'
import { sampleResources } from './resources.mjs'

const [label, proxyPortText, appPortText] = process.argv.slice(2)
if (!/^[a-z0-9-]+$/.test(label ?? '') || !proxyPortText || !appPortText)
  throw new Error('Usage: node scripts/performance/stress.mjs LABEL PROXY_PORT APP_PORT')
const proxyPort = Number(proxyPortText)
const appPort = Number(appPortText)
const origin = 'http://127.0.0.1:' + proxyPort
const output = '.artifacts/performance/'
const password = (await readFile(output + 'fixture-password.local', 'utf8')).trim()
const meter = await startMeter(proxyPort, appPort)
const operations = []

async function signIn(phone, path, field) {
  const result = await fetch(origin + '/api/auth/sign-in/phone', {
    method: 'POST',
    headers: { origin, 'Content-Type': 'application/json' },
    body: JSON.stringify({ phone, password }),
    signal: AbortSignal.timeout(30_000),
  })
  if (!result.ok) throw new Error('Fixture sign-in returned ' + result.status)
  const { user } = await result.json()
  const cookie = result.headers
    .getSetCookie()
    .map((line) => line.split(';', 1)[0])
    .join('; ')
  if (!user?.id || !cookie) throw new Error('Fixture sign-in lacks identity or cookie')
  return { path, field, headers: { cookie, 'X-User-Id': user.id } }
}

async function runLane(lane) {
  for (let iteration = 0; iteration < 8; iteration++) {
    const start = performance.now()
    const result = await fetch(origin + lane.path, {
      headers: lane.headers,
      signal: AbortSignal.timeout(30_000),
    })
    if (!result.ok) throw new Error(lane.path + ' returned ' + result.status)
    const body = await result.json()
    if (!Array.isArray(body[lane.field]) || !body[lane.field].length)
      throw new Error(lane.path + ' omitted fixture data')
    operations.push({
      path: lane.path,
      ms: Math.round(performance.now() - start),
      rows: body[lane.field].length,
    })
  }
}

const stopMeasurement = await sampleResources()
try {
  const teacher = await signIn('+12025551001', '/api/notebook', 'events')
  const learner = await signIn('+12025552001', '/api/student/attendance', 'attendance')
  const administrator = await signIn('+12025551000', '/api/notebook', 'events')
  // Tabs share their signed-in browser session under the ordinary login limit.
  const lanes = [teacher, teacher, teacher, learner, learner, learner, administrator]
  meter.phase('seven-concurrent-readers')
  const runs = await Promise.allSettled(lanes.map(runLane))
  for (const run of runs) if (run.status === 'rejected') throw run.reason
} finally {
  await meter.close()
  const resources = await stopMeasurement()
  await writeFile(
    output + label + '-stress.json',
    JSON.stringify({ operations, requests: meter.samples, resources }, null, 2) + '\n',
  )
}
