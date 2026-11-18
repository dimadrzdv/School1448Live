// One port for both protocols: TLS connections go to the HTTPS server, plain HTTP
// gets a redirect to https:// (phones default to http:// when the scheme is omitted).
//   node https-mux.mjs <listen-port> <https-target-port>
import net from 'node:net'

const listenPort = Number(process.argv[2])
const targetPort = Number(process.argv[3])

net
  .createServer((client) => {
    client.on('error', () => {})
    client.once('data', (first) => {
      client.pause()
      if (first[0] === 0x16) {
        // TLS handshake: pass the raw bytes through.
        const upstream = net.connect(targetPort, '127.0.0.1', () => {
          upstream.write(first)
          client.pipe(upstream).pipe(client)
          client.resume()
        })
        upstream.on('error', () => client.destroy())
        client.on('close', () => upstream.destroy())
        return
      }
      const request = first.toString('latin1')
      const host = /\r\nhost:\s*([^\r\n]+)/i.exec(request)?.[1]?.trim() ?? `localhost:${listenPort}`
      const path = /^[A-Z]+ (\S+)/.exec(request)?.[1] ?? '/'
      client.end(
        `HTTP/1.1 301 Moved Permanently\r\nLocation: https://${host}${path}\r\n` +
          'Content-Length: 0\r\nConnection: close\r\n\r\n',
      )
    })
  })
  .listen(listenPort, '0.0.0.0')
