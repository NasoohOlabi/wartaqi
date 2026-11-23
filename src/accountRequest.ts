export function accountFetch(path: string, userId: string, body?: unknown, keepalive = false) {
  return fetch(`${window.location.origin}${path}`, {
    method: body !== undefined ? 'POST' : 'GET',
    headers: {
      'X-User-Id': userId,
      ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
    },
    ...(body !== undefined ? { body: JSON.stringify(body) } : {}),
    signal: AbortSignal.timeout(15000),
    keepalive,
  })
}
