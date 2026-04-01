/**
 * Integration tests for curl command via the native backend.
 *
 * Uses a worker-thread HTTP server so that ureq's synchronous requests
 * don't block the Node event loop (the native backend does blocking I/O).
 */

import { describe, it, expect } from 'vitest';
import { Bash } from '../src/bash.js';
import { tryLoadNative, createNativeBackend } from '../src/native-loader.js';
import { Worker } from 'node:worker_threads';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const WORKER_PATH = join(__dirname, 'curl-server-worker.cjs');
const hasNativeAddon = await tryLoadNative();

interface WorkerServer {
  worker: Worker;
  port: number;
}

async function startWorkerServer(
  opts: { handler?: string; response?: string } = {},
): Promise<WorkerServer> {
  return new Promise((resolve) => {
    const worker = new Worker(WORKER_PATH, { workerData: opts });
    worker.on('message', (msg: { port?: number }) => {
      if (msg.port) {
        resolve({ worker, port: msg.port });
      }
    });
  });
}

async function stopWorkerServer(ws: WorkerServer): Promise<void> {
  ws.worker.postMessage('close');
  await new Promise<void>((resolve) => {
    ws.worker.on('message', (msg: string) => {
      if (msg === 'closed') resolve();
    });
  });
  await ws.worker.terminate();
}

describe.skipIf(!hasNativeAddon)('curl (native backend)', () => {
  it('curl appears in command list', async () => {
    const bash = await Bash.create(createNativeBackend);
    const names = bash.getCommandNames();
    expect(names).toContain('curl');
  });

  it('curl errors when network disabled', async () => {
    const bash = await Bash.create(createNativeBackend);
    const result = await bash.exec('curl http://example.com');
    expect(result.exitCode).not.toBe(0);
    expect(result.stderr).toContain('network access is disabled');
  });

  it('curl errors when URL not in allowlist', async () => {
    const bash = await Bash.create(createNativeBackend, {
      network: {
        enabled: true,
        allowedUrlPrefixes: ['https://allowed.example.com/'],
      },
    });
    const result = await bash.exec('curl http://evil.com/');
    expect(result.exitCode).not.toBe(0);
    expect(result.stderr).toContain('not allowed');
  });

  it('curl GET returns response body', async () => {
    const ws = await startWorkerServer({ response: 'hello from node server' });
    try {
      const bash = await Bash.create(createNativeBackend, {
        network: {
          enabled: true,
          allowedUrlPrefixes: [`http://127.0.0.1:${ws.port}/`],
        },
      });
      const result = await bash.exec(`curl http://127.0.0.1:${ws.port}/`);
      expect(result.exitCode).toBe(0);
      expect(result.stdout).toBe('hello from node server');
    } finally {
      await stopWorkerServer(ws);
    }
  });

  it('curl POST sends data', async () => {
    const ws = await startWorkerServer({ handler: 'echo_body' });
    try {
      const bash = await Bash.create(createNativeBackend, {
        network: {
          enabled: true,
          allowedUrlPrefixes: [`http://127.0.0.1:${ws.port}/`],
        },
      });
      const result = await bash.exec(
        `curl -X POST -d "test=data" http://127.0.0.1:${ws.port}/`,
      );
      expect(result.exitCode).toBe(0);
      expect(result.stdout).toContain('test=data');
    } finally {
      await stopWorkerServer(ws);
    }
  });

  it('curl -o writes to VFS', async () => {
    const ws = await startWorkerServer({ response: 'saved content' });
    try {
      const bash = await Bash.create(createNativeBackend, {
        network: {
          enabled: true,
          allowedUrlPrefixes: [`http://127.0.0.1:${ws.port}/`],
        },
      });
      const result = await bash.exec(
        `curl -o /result.txt http://127.0.0.1:${ws.port}/`,
      );
      expect(result.exitCode).toBe(0);
      const content = bash.fs.readFileSync('/result.txt');
      expect(content).toBe('saved content');
    } finally {
      await stopWorkerServer(ws);
    }
  });

  it('curl -f fails on HTTP error', async () => {
    const ws = await startWorkerServer({ handler: '404' });
    try {
      const bash = await Bash.create(createNativeBackend, {
        network: {
          enabled: true,
          allowedUrlPrefixes: [`http://127.0.0.1:${ws.port}/`],
        },
      });
      const result = await bash.exec(
        `curl -f http://127.0.0.1:${ws.port}/notfound`,
      );
      expect(result.exitCode).not.toBe(0);
    } finally {
      await stopWorkerServer(ws);
    }
  });
});
