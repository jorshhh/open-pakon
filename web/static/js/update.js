// New build banner (rust/crates/pakon-web/src/update.rs).
// Shown until Later; it comes back at the next launch.
import { $ } from './util.js';

let updateLater = false;

export function showUpdate(u) {
  $('update-bar').classList.toggle('hidden', !u || updateLater);
  if (!u) return;
  $('update-title').textContent = `Pakon ${u.latest} is out`;
  $('update-msg').textContent = `You have ${u.current}. Download the new version, ` +
    'quit Pakon, and drag the new one into Applications to replace it.';
}

$('update-get').addEventListener('click', async () => {
  const d = await fetch('/api/update/open', { method: 'POST' }).then(r => r.json());
  if (!d.opened) window.open(d.url, '_blank', 'noopener');
});
$('update-later').addEventListener('click', () => {
  updateLater = true;
  $('update-bar').classList.add('hidden');
});
