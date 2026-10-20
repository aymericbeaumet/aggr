function key(request) {
  if (request.method !== 'GET' || request.headers.range || /(?:^|\/)__aggr(?:\/|$)/.test(request.url.split('?')[0])) return null;
  return JSON.stringify([request.url, request.headers.accept ?? '', request.headers['accept-encoding'] ?? '']);
}

function normalized(headers = {}) {
  const entries = Array.isArray(headers) ? Array.from({ length: headers.length / 2 }, (_, index) => [headers[index * 2], headers[index * 2 + 1]]) : Object.entries(headers);
  return Object.fromEntries(entries.map(([name, value]) => [name.toLowerCase(), value]));
}

export class ReadinessGate {
  #pending = new Set();
  #closed = false;
  #base = '/';
  #limit;
  #timeout;

  constructor({ maxPending = 64, timeoutMs = 30_000 } = {}) {
    this.#limit = maxPending;
    this.#timeout = timeoutMs;
  }

  #page(request) {
    if (!key(request)) return false;
    const accept = String(request.headers.accept ?? '');
    if (/text\/event-stream|application\/json|audio\/|video\//.test(accept)) return false;
    const path = request.url.split('?')[0];
    if (/^\/@(?:vite|fs|id|aggr)(?:\/|$)/.test(path)) return false;
    const leaf = path.split('/').at(-1);
    if (leaf.includes('.') && !/\.html?$/.test(leaf)) return false;
    if (accept.includes('text/html') || request.headers['sec-fetch-mode'] === 'navigate') return true;
    if (accept !== '' && accept !== '*/*') return false;
    const relative = path.startsWith(this.#base) ? path.slice(this.#base.length) : null;
    return relative !== null && (relative === '' || relative === 'index.html' || relative === '404.html' ||
      /^(?:items|sources|categories|tags|browse|preferences|reading-list|offline|page)(?:\/|$)/.test(relative));
  }

  wait(request, response) {
    if (this.#closed || this.#pending.size >= this.#limit || request.aborted || response.destroyed || !this.#page(request)) return null;
    return new Promise(resolve => {
      let timer;
      const entry = {
        settle: ready => {
          if (!this.#pending.delete(entry)) return;
          clearTimeout(timer);
          request.removeListener('aborted', aborted);
          response.removeListener('close', aborted);
          resolve(ready);
        },
      };
      const aborted = () => entry.settle(false);
      this.#pending.add(entry);
      request.once('aborted', aborted);
      response.once('close', aborted);
      timer = setTimeout(() => entry.settle(false), this.#timeout);
    });
  }

  ready(base = this.#base) {
    this.#base = base;
    for (const entry of this.#pending) entry.settle(true);
  }

  close() {
    this.#closed = true;
    for (const entry of this.#pending) entry.settle(false);
  }
}

/** Retain only complete, bounded responses from the active development generation. */
export class LastGoodResponses {
  #entries = new Map();
  #captures = new WeakMap();
  #bytes = 0;
  #generation = 0;
  #limits;

  constructor({ maxEntries = 64, maxBytes = 16 * 1024 * 1024, maxEntryBytes = 4 * 1024 * 1024 } = {}) {
    this.#limits = { maxEntries, maxBytes, maxEntryBytes };
  }

  capture(request, response) {
    const address = key(request);
    if (!address) return;
    const existing = this.#captures.get(response);
    if (existing) { existing.generation = this.#generation; return; }
    const capture = { generation: this.#generation };
    this.#captures.set(response, capture);
    const chunks = [];
    let bytes = 0;
    let headers = {};
    const append = (chunk, encoding) => {
      if (chunk === null || chunk === undefined || typeof chunk === 'function') return;
      const charset = typeof encoding === 'string' ? encoding : 'utf8';
      bytes += typeof chunk === 'string' ? Buffer.byteLength(chunk, charset) : chunk.byteLength;
      if (bytes > Math.min(this.#limits.maxEntryBytes, this.#limits.maxBytes)) { chunks.length = 0; return; }
      chunks.push(typeof chunk === 'string' ? Buffer.from(chunk, charset) : Buffer.from(chunk));
    };
    const writeHead = response.writeHead;
    response.writeHead = function (status, ...arguments_) {
      const explicit = arguments_.at(-1);
      headers = { ...normalized(this.getHeaders()), ...normalized(typeof explicit === 'object' ? explicit : {}) };
      return writeHead.call(this, status, ...arguments_);
    };
    const write = response.write;
    response.write = function (chunk, ...arguments_) {
      append(chunk, arguments_[0]);
      return write.call(this, chunk, ...arguments_);
    };
    const end = response.end;
    response.end = function (chunk, ...arguments_) {
      append(chunk, arguments_[0]);
      return end.call(this, chunk, ...arguments_);
    };
    response.once('finish', () => {
      if (capture.generation !== this.#generation || response.statusCode !== 200 ||
        String(headers['content-type'] ?? '').includes('text/event-stream') ||
        headers['set-cookie'] || bytes > this.#limits.maxEntryBytes || bytes > this.#limits.maxBytes) return;
      const previous = this.#entries.get(address);
      if (previous) this.#bytes -= previous.body.length;
      this.#entries.delete(address);
      this.#entries.set(address, { headers, body: Buffer.concat(chunks) });
      this.#bytes += bytes;
      while (this.#entries.size > this.#limits.maxEntries || this.#bytes > this.#limits.maxBytes) {
        const oldest = this.#entries.keys().next().value;
        this.#bytes -= this.#entries.get(oldest).body.length;
        this.#entries.delete(oldest);
      }
    });
  }

  respond(request, response) {
    const address = key(request);
    const saved = address && this.#entries.get(address);
    if (!saved) return false;
    this.#entries.delete(address);
    this.#entries.set(address, saved);
    const headers = { ...saved.headers, 'content-length': saved.body.length, 'cache-control': 'no-store' };
    delete headers['transfer-encoding'];
    delete headers.connection;
    response.writeHead(200, headers);
    response.end(saved.body);
    return true;
  }

  clear() {
    this.#generation++;
    this.#entries.clear();
    this.#bytes = 0;
  }
}
