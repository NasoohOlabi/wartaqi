import { createServer, request as httpRequest } from 'node:http'
import { connect } from 'node:net'
import { once } from 'node:events'

function headerBlock(start, rawHeaders) {
  let lines = start + '\r\n'
  for (let index = 0; index < rawHeaders.length; index += 2)
    lines += rawHeaders[index] + ': ' + rawHeaders[index + 1] + '\r\n'
  return lines + '\r\n'
}

export async function startMeter(listenPort, upstreamPort) {
  const samples = []
  const sockets = new Set()
  let phase = 'setup'

  function sample(request) {
    const url = new URL(request.url, 'http://127.0.0.1')
    const record = {
      phase,
      method: request.method,
      path: url.pathname,
      ...(url.pathname === '/api/notebook' && {
        view: url.searchParams.get('focusView') ?? 'full',
      }),
      sent: Buffer.byteLength(
        headerBlock(request.method + ' ' + request.url + ' HTTP/1.1', request.rawHeaders),
      ),
      received: 0,
      status: 0,
      finished: false,
    }
    samples.push(record)
    return record
  }

  const server = createServer((request, reply) => {
    const record = sample(request)
    const upstream = httpRequest(
      {
        hostname: '127.0.0.1',
        port: upstreamPort,
        method: request.method,
        path: request.url,
        headers: request.headers,
      },
      (response) => {
        record.status = response.statusCode
        record.received = Buffer.byteLength(
          headerBlock(
            'HTTP/1.1 ' + response.statusCode + ' ' + response.statusMessage,
            response.rawHeaders,
          ),
        )
        reply.writeHead(response.statusCode, response.rawHeaders)
        response.on('data', (chunk) => (record.received += chunk.length))
        response.on('error', () => reply.destroy())
        response.pipe(reply)
      },
    )
    request.on('data', (chunk) => (record.sent += chunk.length))
    request.on('error', () => upstream.destroy())
    reply.on('finish', () => (record.finished = true))
    reply.on('close', () => {
      if (!reply.writableFinished) upstream.destroy()
    })
    upstream.on('error', () => {
      if (reply.headersSent) reply.destroy()
      else reply.writeHead(502).end()
    })
    request.pipe(upstream)
  })

  server.on('connection', (socket) => {
    sockets.add(socket)
    socket.on('close', () => sockets.delete(socket))
  })
  server.on('upgrade', (request, browser, head) => {
    const record = sample(request)
    if (process.env.WARTAQ_PERF_BLACKHOLE_WS === '1') return
    const upstream = connect(upstreamPort, '127.0.0.1')
    upstream.on('error', () => browser.destroy())
    browser.on('error', () => upstream.destroy())
    browser.on('close', () => upstream.destroy())
    upstream.on('close', () => browser.destroy())
    upstream.write(headerBlock('GET ' + request.url + ' HTTP/1.1', request.rawHeaders))
    if (head.length) upstream.write(head)
    upstream.once('data', (chunk) => {
      record.status = Number(chunk.toString('ascii', 0, 16).split(' ')[1])
      record.finished = record.status === 101
    })
    // Include the upgrade response and frame bytes on this one connection.
    upstream.on('data', (chunk) => (record.received += chunk.length))
    browser.on('data', (chunk) => (record.sent += chunk.length))
    browser.pipe(upstream).pipe(browser)
  })

  server.listen(listenPort, '127.0.0.1')
  await once(server, 'listening')
  return {
    samples,
    phase(name) {
      phase = name
    },
    async close() {
      for (const socket of sockets) socket.destroy()
      await new Promise((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      )
    },
  }
}
