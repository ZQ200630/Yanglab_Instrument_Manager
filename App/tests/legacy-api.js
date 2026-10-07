/** Test-only retired v2 bridge: never imported by the production frontend. */
export function createClient(invoke) {
  if (typeof invoke !== 'function') throw new TypeError('Tauri invoke is required');
  if (typeof globalThis.crypto?.getRandomValues !== 'function') {
    throw new Error('Secure request ID generation is unavailable');
  }
  const nonce = new Uint32Array(4);
  globalThis.crypto.getRandomValues(nonce);
  const namespace = Array.from(nonce, (word) => word.toString(16).padStart(8, '0')).join('');
  let sequence = 0;
  const queries = new Map();

  return Object.freeze({
    start({ mode = 'real', pythonPath }) {
      if (mode !== 'real') throw new Error('Only real instrument execution is supported');
      if (typeof pythonPath !== 'string' || !pythonPath.trim()) {
        throw new Error('Select the Anaconda VISA Python interpreter');
      }
      return invoke('worker_start', { config: { mode, pythonPath } });
    },
    request(method, params = {}, context = null) {
      if (!['status', 'ping'].includes(method)) return send(method, params, context);
      const existing = queries.get(method);
      const key = JSON.stringify({ params, context });
      if (existing) {
        if (existing.key === key) return existing.promise;
        return Promise.reject(Object.assign(new Error('Query context changed while an earlier query is pending'),
          { phase: 'rejected_before_call', context, transportUnknown: false }));
      }
      const previous = [...queries.values()].at(-1);
      const promise = (previous ? previous.promise.then(() => {}, () => {}).then(() => send(method, params, context))
        : send(method, params, context)).finally(() => queries.delete(method));
      queries.set(method, { key, promise });
      return promise;
    },
    stop() {
      return invoke('worker_stop');
    },
  });

  async function send(method, params, context) {
      if (typeof method !== 'string' || !method) throw new Error('Method is required');
      if (!params || typeof params !== 'object' || Array.isArray(params)) {
        throw new Error('Parameters must be an object');
      }
      const id = `ui-${namespace}-${++sequence}`;
      let response;
      try {
        response = await invoke('worker_request', { request: { v: 2, id, method, params, context } });
      } catch (cause) {
        throw Object.assign(new Error(cause?.message || String(cause)),
          { requestId: id, phase: null, context, transportUnknown: true });
      }
      if (!response || response.v !== 2 || response.id !== id) {
        throw Object.assign(new Error('Worker response ID or version mismatch; device outcome is unknown'),
          { requestId: id, phase: null, context, transportUnknown: true });
      }
      if (!response.ok) {
        const error = response.error || {};
        throw Object.assign(new Error(`${error.type || 'InstrumentError'}: ${error.message || 'operation failed'}`),
          { phase: response.phase, context: response.context, requestId: id, type: error.type });
      }
      if (response.phase !== 'completed' || !Object.hasOwn(response, 'context')) {
        throw Object.assign(new Error('Invalid terminal response; outcome unknown'), { requestId: id });
      }
      return { requestId: id, phase: response.phase, context: response.context, result: response.result };
  }
}

export function tauriClient() {
  const invoke = globalThis.__TAURI__?.core?.invoke;
  if (!invoke) throw new Error('Open this console in the Tauri desktop window');
  return createClient(invoke);
}

export async function probeExistingWorker(client) {
  try {
    return (await client.request('status')).result;
  } catch (error) {
    // Tauri serializes Rust Result::Err(String) as a rejected string, not Error.
    if (/worker is not running/i.test(error?.message || String(error))) return null;
    throw error;
  }
}
