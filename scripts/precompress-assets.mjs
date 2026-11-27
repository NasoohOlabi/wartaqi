import { readdir, readFile, writeFile, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { brotliCompressSync, constants, gzipSync } from 'node:zlib'

const root = process.argv[2] ?? 'dist'
const dir = join(root, 'assets')
for (const name of await readdir(dir)) {
  if (!/-[A-Za-z0-9_-]{8}\.(js|css)$/.test(name)) continue
  const file = join(dir, name)
  const bytes = await readFile(file)
  for (const [suffix, compress] of [
    ['.br', () => brotliCompressSync(bytes, { params: { [constants.BROTLI_PARAM_QUALITY]: 6 } })],
    ['.gz', () => gzipSync(bytes)],
  ]) {
    const output = file + suffix
    const compressed = bytes.length >= 1024 ? compress() : bytes
    if (compressed.length < bytes.length) await writeFile(output, compressed)
    else await rm(output, { force: true })
  }
}
