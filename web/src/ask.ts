// A one-line text dialog in the page, in place of prompt(): the macOS app
// window (wry's WKWebView) has no prompt() and returns null at once.
import { reactive } from 'vue';

export interface AskOptions {
  value?: string;
  label?: string;
  ok?: string;
  check?: ((v: string) => string) | null;   // a message refuses the value
}

export const asking = reactive({
  open: false,
  value: '',
  label: '',
  ok: 'Set',
  check: null as AskOptions['check'],
  done: null as ((v: string | null) => void) | null,
});

// Resolves to the trimmed value, or null on Cancel, Esc or a click outside.
export function ask({ value = '', label = '', ok = 'Set', check = null }: AskOptions = {}): Promise<string | null> {
  if (asking.done) finishAsk(null);
  return new Promise(resolve => Object.assign(asking, { open: true, value, label, ok, check, done: resolve }));
}

export function finishAsk(v: string | null) {
  const d = asking.done;
  Object.assign(asking, { open: false, done: null });
  d?.(v);
}
