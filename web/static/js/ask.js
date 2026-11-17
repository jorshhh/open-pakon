// A one-line text dialog in the page, in place of prompt(): the macOS app
// window (wry's WKWebView) has no prompt() and returns null at once.
import { $, setMsg } from './util.js';

let done = null;      // resolves the open ask()
let check = null;     // value -> error message, '' when fine

function finish(v) {
  $('ask-modal').classList.add('hidden');
  const d = done;
  done = null;
  if (d) d(v);
}

// Ask for a line of text. Resolves to the trimmed value, or null on Cancel,
// Esc or a click outside. `check` may refuse a value with a message.
export function ask({ value = '', label = '', ok = 'Set', check: c = null } = {}) {
  if (done) finish(null);
  check = c;
  $('ask-form').setAttribute('aria-label', label);
  $('ask-input').setAttribute('aria-label', label);
  $('ask-input').value = value;
  $('ask-ok').textContent = ok;
  setMsg($('ask-msg'), '');
  $('ask-modal').classList.remove('hidden');
  $('ask-input').focus();
  $('ask-input').select();
  return new Promise(resolve => { done = resolve; });
}

$('ask-form').addEventListener('submit', e => {
  e.preventDefault();
  const v = $('ask-input').value.trim();
  const err = check ? check(v) : '';
  if (err) { setMsg($('ask-msg'), err, 'err'); return; }
  finish(v);
});
$('ask-input').addEventListener('input', () => setMsg($('ask-msg'), ''));
$('ask-cancel').addEventListener('click', () => finish(null));
$('ask-modal').addEventListener('click', e => { if (e.target === $('ask-modal')) finish(null); });
// capture, so Esc closes this dialog and not the folder picker under it
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && done) { e.stopImmediatePropagation(); finish(null); }
}, true);
