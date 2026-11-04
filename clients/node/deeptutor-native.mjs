/** Dependency-free client of an already running, local Rust service (Node 22+). */
import http from 'node:http';
import {randomUUID} from 'node:crypto';
import {setTimeout as sleep} from 'node:timers/promises';

const MAX_RESPONSE_BYTES = 32 * 1024 * 1024;
const positive = (value, name) => {
  if (!Number.isSafeInteger(value) || value <= 0) throw new TypeError(`${name} must be a positive integer`);
  return value;
};
const identifier = value => {
  if (typeof value !== 'string' || !/^[a-zA-Z0-9_-]{1,256}$/.test(value)) throw new TypeError('Invalid resource ID');
  return value;
};
const stableId = (value, name) => {
  if (typeof value !== 'string' || !value.trim()) throw new TypeError(`Use a stable, nonempty ${name}, including on retries`);
  return value;
};
const cursorValue = value => {
  if (!Number.isSafeInteger(value) || value < 0) throw new TypeError('afterSeq must be a nonnegative integer');
  return value;
};

export class NativeError extends Error {
  constructor(status, payload) {
    super(String(payload?.message || payload?.detail || `HTTP ${status}`));
    this.name = 'NativeError';
    this.status = status;
    this.payload = payload;
  }
}

export class NativeClient {
  constructor(baseUrl = 'http://127.0.0.1:8002', {workspaceId = '', timeoutMs = 30_000} = {}) {
    const url = new URL(baseUrl);
    if (url.protocol !== 'http:' || !['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)
      || url.username || url.password || url.search || url.hash || url.pathname !== '/') {
      throw new TypeError('Use the HTTP origin of the local Rust service');
    }
    if (typeof workspaceId !== 'string' || workspaceId.length > 256) throw new TypeError('Invalid workspace ID');
    this.baseUrl = url.origin;
    this.workspaceId = workspaceId;
    this.timeoutMs = positive(timeoutMs, 'timeoutMs');
  }

  async #request(method, path, {body, query = {}, signal, timeoutMs = this.timeoutMs} = {}) {
    const url = new URL(path, this.baseUrl);
    for (const [key, value] of Object.entries({...query, workspace_id: this.workspaceId})) url.searchParams.set(key, String(value));
    const data = body === undefined ? undefined : Buffer.from(JSON.stringify(body));
    const deadline = AbortSignal.timeout(positive(timeoutMs, 'timeoutMs'));
    const requestSignal = signal ? AbortSignal.any([signal, deadline]) : deadline;
    // node:http neither follows redirects nor routes local requests through environment proxies.
    return new Promise((resolve, reject) => {
      const req = http.request(url, {method, signal: requestSignal, headers: {
        Accept: 'application/json', ...(data ? {'Content-Type': 'application/json', 'Content-Length': data.length} : {}),
      }}, response => {
        const buffers = [];
        let size = 0;
        response.on('data', chunk => {
          size += chunk.length;
          if (size > MAX_RESPONSE_BYTES) response.destroy(new Error('Native response exceeds 32 MiB'));
          else buffers.push(chunk);
        });
        response.on('error', reject);
        response.on('end', () => {
          const status = response.statusCode ?? 0;
          let payload;
          try { payload = JSON.parse(Buffer.concat(buffers).toString('utf8')); }
          catch {
            reject(status < 200 || status >= 300 ? new NativeError(status, {}) : new Error('Invalid native JSON response'));
            return;
          }
          if (status < 200 || status >= 300) reject(new NativeError(status, payload));
          else resolve(payload);
        });
      });
      req.on('error', reject);
      req.end(data);
    });
  }

  start(content, {submissionId, capability = 'chat', sessionId = null, knowledgeBases = [], signal} = {}) {
    stableId(submissionId, 'submissionId');
    return this.#request('POST', '/api/turns', {signal, body: {
      content, client_submission_id: submissionId, capability, session_id: sessionId, knowledge_bases: knowledgeBases,
    }});
  }

  async events(turnId, {afterSeq = 0, signal, timeoutMs} = {}) {
    cursorValue(afterSeq);
    const result = await this.#request('GET', `/api/turns/${identifier(turnId)}/events`, {
      signal, timeoutMs, query: {after_seq: afterSeq},
    });
    if (!Array.isArray(result.events)) throw new Error('Invalid native event page');
    return result.events;
  }

  async *stream(turnId, {afterSeq = 0, pollIntervalMs = 100, waitTimeoutMs = 180_000, signal} = {}) {
    let cursor = cursorValue(afterSeq);
    positive(pollIntervalMs, 'pollIntervalMs');
    const deadline = performance.now() + positive(waitTimeoutMs, 'waitTimeoutMs');
    while (performance.now() < deadline) {
      signal?.throwIfAborted();
      const remaining = Math.max(1, Math.ceil(deadline - performance.now()));
      const events = await this.events(turnId, {afterSeq: cursor, signal, timeoutMs: Math.min(this.timeoutMs, remaining)});
      for (const event of events) {
        if (event.seq !== cursor + 1) throw new Error('Noncontiguous durable event sequence');
        cursor = event.seq;
        yield event;
        if (event.type === 'done' || event.type === 'wait_for_input') return;
      }
      if (!events.length) await sleep(Math.min(pollIntervalMs, Math.max(1, deadline - performance.now())), undefined, {signal});
    }
    throw new Error(`Timed out waiting for turn ${turnId}; resume after seq ${cursor}`);
  }

  reply(turnId, {toolCallId, text, commandId, signal} = {}) {
    stableId(commandId, 'commandId');
    return this.#request('POST', `/api/turns/${identifier(turnId)}/reply`, {signal, body: {
      tool_call_id: toolCallId, text, command_id: commandId,
    }});
  }

  cancel(turnId, {commandId, signal} = {}) {
    stableId(commandId, 'commandId');
    return this.#request('POST', `/api/turns/${identifier(turnId)}/cancel`, {signal, body: {command_id: commandId}});
  }

  async sessions({signal} = {}) {
    return (await this.#request('GET', '/api/sessions', {signal})).sessions;
  }

  async *run(content, {submissionId = randomUUID(), ...options} = {}) {
    const receipt = await this.start(content, {submissionId, ...options});
    yield* this.stream(receipt.turn_id, {signal: options.signal});
  }
}
