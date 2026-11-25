import { parseArgs } from 'node:util'
import { createAgent, type AgentModel } from '../telegram/runtime/agent'
import { createAgentModel } from '../telegram/runtime/agent-model'
import { labCases, syntheticAccess, type LabCase } from './agent-lab-fixtures'

export async function runLabCase(fixture: LabCase, live?: AgentModel) {
  const calls: string[] = []
  let modelCalls = 0
  const model: AgentModel | undefined =
    live ?? (fixture.plan === undefined ? undefined : async () => fixture.plan)
  const run = createAgent(
    model &&
      (async (...args) => {
        modelCalls++
        return model(...args)
      }),
  )
  const result = await run({
    question: fixture.question,
    lang: fixture.lang ?? 'en',
    access: syntheticAccess(fixture.role ?? 'guest', fixture.data ?? 'normal', calls),
  })
  const ok =
    result.source === fixture.expected &&
    (!fixture.includes || result.text.includes(fixture.includes)) &&
    calls.join() === (fixture.calls ?? []).join()
  return { ...result, calls, modelCalls, ok }
}

if (import.meta.main) {
  const { positionals, values } = parseArgs({
    allowPositionals: true,
    options: {
      live: { type: 'boolean' },
      json: { type: 'boolean' },
      ar: { type: 'boolean' },
      role: { type: 'string', default: 'guest' },
      model: { type: 'string', default: 'prism-ml/bonsai-27b' },
    },
  })
  const role = values.role
  if (role !== 'guest' && role !== 'student' && role !== 'staff')
    throw new Error('Choose --role guest, student or staff.')
  const model = values.live
    ? createAgentModel({
        url: 'http://127.0.0.1:8081/v1/chat/completions',
        model: values.model,
        key: '',
      })
    : undefined
  if (positionals.length > 0) {
    const calls: string[] = []
    const result = await createAgent(model)({
      question: positionals.join(' '),
      lang: values.ar ? 'ar' : 'en',
      access: syntheticAccess(role, 'normal', calls),
    })
    console.info(values.json ? JSON.stringify({ ...result, calls }) : result.text)
  } else {
    let passed = 0
    let livePassed = 0
    let liveCount = 0
    for (const fixture of labCases) {
      const live = fixture.live ? model : undefined
      const result = await runLabCase(fixture, live)
      if (result.ok) passed++
      if (live) {
        liveCount += result.modelCalls
        if (result.ok) livePassed += result.modelCalls
      }
      console.info(
        values.json
          ? JSON.stringify({ id: fixture.id, question: fixture.question, ...result })
          : `${result.ok ? 'PASS' : 'FAIL'} ${fixture.id} (${result.source}${result.reason ? `, ${result.reason}` : ''})${!result.ok ? `: ${result.text}` : ''}`,
      )
    }
    const report = values.json ? console.error : console.info
    report(`${passed}/${labCases.length} cases passed.`)
    if (model) report(`${livePassed}/${liveCount} real LM Studio tool selections passed.`)
    if (passed !== labCases.length) process.exitCode = 1
  }
}
