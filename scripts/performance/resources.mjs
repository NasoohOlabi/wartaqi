import { execFile } from 'node:child_process'
import { promisify } from 'node:util'

const exec = promisify(execFile)

async function containerSnapshot(container) {
  const linux = [
    'cat /sys/fs/cgroup/cpu.stat',
    'printf "memory_bytes "; cat /sys/fs/cgroup/memory.current',
    'printf "peak_bytes "; cat /sys/fs/cgroup/memory.peak',
    'grep -E "^(anon|inactive_file) " /sys/fs/cgroup/memory.stat',
  ].join('; ')
  const { stdout } = await exec('docker', ['exec', container, 'sh', '-ec', linux])
  return Object.fromEntries(
    stdout
      .trim()
      .split('\n')
      .map((line) => {
        const [key, value] = line.trim().split(' ')
        return [key, Number(value)]
      }),
  )
}

export async function sampleResources() {
  const web = process.env.WARTAQ_PERF_WEB
  const database = process.env.WARTAQ_PERF_DB
  const samples = []
  let stopped = false
  let timer
  let current
  async function sample() {
    const names = { web, database }
    const containers = await Promise.all(
      Object.entries(names).flatMap(([key, name]) =>
        name ? [containerSnapshot(name).then((value) => [key, value])] : [],
      ),
    )
    samples.push({ time: Date.now(), ...Object.fromEntries(containers) })
  }
  function schedule() {
    if (stopped || (!web && !database)) return
    timer = setTimeout(() => {
      current = sample().finally(schedule)
      // Missing measurements still fail stop(), without an unhandled rejection.
      void current.catch(() => {})
    }, 1000)
  }
  await sample()
  schedule()
  return async () => {
    stopped = true
    clearTimeout(timer)
    await current
    await sample()
    return samples
  }
}
