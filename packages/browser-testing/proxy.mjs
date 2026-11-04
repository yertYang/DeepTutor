// Enforce origins at the network boundary, including every HTTP redirect and
// TLS/WebSocket connection. Page routing alone does not intercept redirect hops.
import http from 'node:http';
import net from 'node:net';
import {once} from 'node:events';
import {allowedUrl} from './scenario.mjs';

export async function testProxy(config) {
  const sockets = new Set();
  const track = socket => { sockets.add(socket); socket.once('close', () => sockets.delete(socket)); return socket; };
  function allowed(value, scheme) {
    const url = new URL(value);
    if (scheme) url.protocol = scheme;
    allowedUrl(url.href, config.base_url, config.allowed_origins);
    if ((config.blocked_ports ?? []).includes(Number(url.port || (url.protocol === 'https:' ? 443 : 80)))) throw new Error('The application control service cannot be tested with this browser.');
    return url;
  }
  const server = http.createServer((req, res) => {
    let url;
    try { url = allowed(req.url); if (url.protocol !== 'http:') throw new Error('Use CONNECT for HTTPS.'); }
    catch { res.writeHead(403).end('Website outside the configured access scope.'); return; }
    const headers = {...req.headers, host: url.host}; delete headers['proxy-connection']; delete headers['proxy-authorization'];
    const upstream = http.request(url, {method: req.method, headers}, response => {
      res.writeHead(response.statusCode, response.headers); response.pipe(res);
    });
    upstream.on('socket', track);
    upstream.on('error', () => { if (!res.headersSent) res.writeHead(502); res.end('Test website could not be reached.'); });
    req.on('aborted', () => upstream.destroy());
    res.on('close', () => { if (!res.writableFinished) upstream.destroy(); });
    req.pipe(upstream);
  });
  server.on('connection', track);
  server.on('connect', (req, client, head) => {
    let url;
    try { url = allowed(`https://${req.url}`); }
    catch { client.end('HTTP/1.1 403 Forbidden\r\n\r\n'); return; }
    const upstream = track(net.connect({host: url.hostname.replace(/^\[|\]$/g, ''), port: Number(url.port || 443)}, () => {
      client.write('HTTP/1.1 200 Connection Established\r\n\r\n');
      if (head.length) upstream.write(head);
      client.pipe(upstream); upstream.pipe(client);
    }));
    client.on('error', () => upstream.destroy()); client.on('close', () => upstream.destroy());
    upstream.on('error', () => client.destroy()); upstream.on('close', () => client.destroy());
  });
  server.on('upgrade', (req, client, head) => {
    let url;
    try { url = allowed(req.url, 'http:'); }
    catch { client.destroy(); return; }
    const upstream = http.request(url, {headers: {...req.headers, host: url.host}});
    upstream.on('socket', track);
    upstream.on('upgrade', (res, socket, responseHead) => {
      client.write(`HTTP/1.1 ${res.statusCode} ${res.statusMessage}\r\n${Object.entries(res.headers).map(([k, v]) => `${k}: ${v}`).join('\r\n')}\r\n\r\n`);
      if (head.length) socket.write(head); if (responseHead.length) client.write(responseHead);
      client.pipe(socket); socket.pipe(client);
      client.on('close', () => socket.destroy()); socket.on('close', () => client.destroy());
    });
    upstream.on('error', () => client.destroy()); upstream.end();
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    async close() { for (const socket of sockets) socket.destroy(); await new Promise(resolve => server.close(resolve)); },
  };
}
