// The order the next scan goes into: the next number from the counter, or
// one the operator sets, e.g. the lab's order sticker on the film (#95).
// Set in a small dialog (cleared = back to the next number); used up by
// the scan. Cancel keeps what was there.
import { $ } from './util.js';
import { ask } from './ask.js';

let next = '';       // the counter's next number (from /api/status)
let custom = '';     // the operator's order, '' = the next number

function show() {
  $('scan-order').textContent = custom || next || '—';
  $('scan-order').title = custom ? 'Set for the next scan' : 'The next number';
}

export function showNextOrder(n) {
  if (n) { next = n; show(); }
}

// The order to send with a scan ('' = the counter).
export function scanOrder() { return custom; }

// The scan has its folder: the next one goes back to the counter.
export function orderUsed() { custom = ''; show(); }

// The same rules as the service (orders::custom_order), checked here so the
// dialog can say what is wrong before a scan starts.
function problem(s) {
  if (!s) return '';
  if (/[^\p{L}\p{N} ._-]/u.test(s)) return 'Use letters, digits, spaces, - _ .';
  if (s.startsWith('.') || s.endsWith('.')) return "Can't start or end with “.”";
  const stem = s.split('.')[0].trimEnd().toUpperCase();
  if (/^(CON|PRN|AUX|NUL|COM\d|LPT\d)$/.test(stem)) return 'Not allowed as a folder name on Windows';
  return '';
}

$('scan-order-btn').addEventListener('click', async () => {
  const v = await ask({ value: custom || next, label: 'Order number', check: problem });
  if (v === null) return;          // cancelled: keep what was there
  custom = v === next ? '' : v;
  show();
});
