export function newEditVersion() {
  return { time: new Date().toISOString(), id: crypto.randomUUID() }
}

export function accountFetch(
  path: string,
  userId: string,
  body?: unknown,
  edit?: { time: string; id: string },
) {
  return fetch(`${window.location.origin}${path}`, {
    method: body !== undefined ? 'POST' : 'GET',
    headers: {
      'X-User-Id': userId,
      ...(edit ? { 'X-Edit-Time': edit.time, 'X-Edit-Id': edit.id } : {}),
      ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
    },
    ...(body !== undefined ? { body: JSON.stringify(body) } : {}),
    signal: AbortSignal.timeout(15000),
  })
}
