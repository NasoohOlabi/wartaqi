export function newEditVersion() {
  return { time: new Date().toISOString(), id: crypto.randomUUID() }
}

export function accountFetch(
  path: string,
  userId: string,
  body?: unknown,
  edit?: { time: string; id: string },
  validator?: string,
  signal?: AbortSignal,
) {
  const timeout = AbortSignal.timeout(15000)
  return fetch(`${window.location.origin}${path}`, {
    method: body !== undefined ? 'POST' : 'GET',
    headers: {
      'X-User-Id': userId,
      ...(validator ? { 'If-None-Match': validator } : {}),
      ...(edit ? { 'X-Edit-Time': edit.time, 'X-Edit-Id': edit.id } : {}),
      ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
    },
    ...(body !== undefined ? { body: JSON.stringify(body) } : {}),
    signal: signal ? AbortSignal.any([signal, timeout]) : timeout,
  })
}
