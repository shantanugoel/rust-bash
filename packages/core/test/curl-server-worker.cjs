/**
 * Worker thread that runs a local HTTP test server.
 *
 * workerData options:
 *   handler: 'default' | 'echo_body' | '404'
 *   response: string (used by 'default' handler)
 *
 * Protocol:
 *   Worker → Parent: { port: number }  (once listening)
 *   Parent → Worker: 'close'           (request shutdown)
 *   Worker → Parent: 'closed'          (shutdown complete)
 */

const { parentPort, workerData } = require('worker_threads');
const http = require('http');

const handler = workerData?.handler || 'default';
const responseBody = workerData?.response || 'hello';

const server = http.createServer((req, res) => {
  if (handler === 'echo_body') {
    let body = '';
    req.on('data', (chunk) => (body += chunk.toString()));
    req.on('end', () => {
      res.writeHead(200, { 'Content-Type': 'text/plain' });
      res.end(body);
    });
  } else if (handler === '404') {
    res.writeHead(404, { 'Content-Type': 'text/plain' });
    res.end('Not Found');
  } else {
    res.writeHead(200, { 'Content-Type': 'text/plain' });
    res.end(responseBody);
  }
});

server.listen(0, '127.0.0.1', () => {
  parentPort.postMessage({ port: server.address().port });
});

parentPort.on('message', (msg) => {
  if (msg === 'close') {
    server.close(() => parentPort.postMessage('closed'));
  }
});
