// Talking to the local service (rust/crates/pakon-web): JSON posts and the
// event streams long jobs report on.

// POST `body` as JSON; resolves to the Response.
export function postJSON(url: string, body: unknown): Promise<Response> {
  return fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

// One event of a job's stream: `type` is log, progress, done, error, or a
// job's own (order, feed); the rest depends on it.
export interface StreamEvent {
  type: string;
  message?: string;
  [key: string]: any;
}

// POST `body` and read the server-sent events until the job ends. A request
// refused before the stream starts (400 etc.) arrives as one error event.
export async function postSSE(url: string, body: unknown, onEvent: (ev: StreamEvent) => void): Promise<void> {
  const resp = await postJSON(url, body);
  if (!resp.ok || !resp.body) {
    const d = await resp.json().catch(() => ({}));
    onEvent({ type: 'error', message: d.error || d.detail || `${resp.status} ${resp.statusText}` });
    return;
  }
  const reader = resp.body.getReader();
  const dec = new TextDecoder();
  let buf = '';
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    const parts = buf.split('\n\n');
    buf = parts.pop() ?? '';
    for (const part of parts) {
      const line = part.trim();
      if (line.startsWith('data: ')) {
        try { onEvent(JSON.parse(line.slice(6))); } catch { /* a partial or foreign line */ }
      }
    }
  }
}

// GET JSON.
export const getJSON = <T = any>(url: string): Promise<T> => fetch(url).then(r => r.json());

// localStorage that never throws (private windows, blocked storage).
export const store = {
  get(key: string): string | null { try { return localStorage.getItem(key); } catch { return null; } },
  set(key: string, value: string) { try { localStorage.setItem(key, value); } catch { /* not kept */ } },
};

// Page switches and page errors go to the local service, which sends them
// only when the user agreed (and never from a checkout).
export function report(kind: 'page' | 'error', body: unknown) {
  fetch('/api/telemetry/' + kind, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body), keepalive: true,
  }).catch(() => {});
}
