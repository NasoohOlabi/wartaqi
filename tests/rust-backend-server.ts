import { z } from 'zod'

const health = z.object({ ok: z.literal(true) })

export async function startRustBackend(binary: string, databaseUrl: string, bunOrigin: string) {
  const child = Bun.spawn([binary], {
    env: {
      ...process.env,
      PORT: '0',
      BUN_ORIGIN: bunOrigin,
      WORKSPACE_ORIGIN: bunOrigin,
      DATABASE_URL: databaseUrl,
    },
    stdout: 'pipe',
    stderr: 'inherit',
  })
  const deadline = AbortSignal.timeout(10000)
  try {
    const ready = async () => {
      let output = ''
      for await (const chunk of child.stdout.pipeThrough(new TextDecoderStream())) {
        output += chunk
        const match = /^Listening on (http:\/\/127\.0\.0\.1:\d+)\r?$/m.exec(output)
        if (match) return match[1]
      }
      throw new Error('Rust backend exited before publishing its address')
    }
    const origin = await Promise.race([
      ready(),
      new Promise<never>((_, reject) => {
        deadline.addEventListener('abort', () => reject(deadline.reason), { once: true })
      }),
    ])
    const response = await fetch(`${origin}/health`, { signal: deadline })
    if (!response.ok) throw new Error(`Rust backend health check returned ${response.status}`)
    health.parse(await response.json())
    return { origin, child }
  } catch (error) {
    child.kill()
    await child.exited
    throw error
  }
}
