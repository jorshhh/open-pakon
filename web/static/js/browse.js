// Picking the work folder (each scan is an order folder in it) or a .raw
// that is already on the host: the system's own dialog in the app window
// (/api/pick), else the page's folder picker (/api/browse) in a browser.
import { $, setMsg, postJSON } from './util.js';
import { showSpace } from './spaces.js';
import { showRaw, processRaw } from './process.js';

let workDir = null;      // work folder the order folders go into (host)
let browsePath = null;   // dir currently shown in the modal
let browseRaws = false;  // picking a .raw to open, not the work folder
let nativePicker = false; // the app window shows the system's dialog

export function setNativePicker(on) { nativePicker = !!on; }

export function showWorkDir(dir, nextOrder) {
  if (dir) { workDir = dir; $('scan-out').textContent = dir; $('scan-out').title = dir; }
  if (nextOrder) $('scan-order').textContent = nextOrder;
}

// The ways in: the work folder (Scan), a raw (Process, Home).
function pick(raws) {
  if (nativePicker) nativePick(raws);
  else openBrowser(raws, workDir);
}

async function nativePick(raws) {
  const d = await postJSON('/api/pick', { kind: raws ? 'raw' : 'folder', start: workDir })
    .then(r => r.json()).catch(() => ({}));
  if (!d.path) return;                     // cancelled
  if (raws) {
    openRaw(d.path, err => { showSpace('process'); setMsg($('proc-msg'), err, 'err'); });
  } else {
    useWorkDir(d.path, err => setMsg($('scan-msg'), err, 'err'));
  }
}

function openBrowser(raws, start) {
  browseRaws = raws;
  $('browse-title').textContent = raws ? 'Open a .raw' : 'Choose work folder';
  $('browse-new').classList.toggle('hidden', raws);
  $('browse-use').classList.toggle('hidden', raws);
  $('browse-modal').classList.remove('hidden');
  browseTo(start);
}
const closeBrowser = () => $('browse-modal').classList.add('hidden');

function browseItem(icon, name, onclick) {
  const it = document.createElement('div');
  it.className = 'browse-item';
  it.innerHTML = `<span class="ic">${icon}</span><span></span>`;
  it.querySelector('span:last-child').textContent = name;
  it.onclick = onclick;
  return it;
}

async function browseTo(path) {
  setMsg($('browse-msg'), '');
  const q = new URLSearchParams();
  if (path) q.set('path', path);
  if (browseRaws) q.set('raws', 'true');
  const d = await fetch('/api/browse?' + q).then(r => r.json());
  browsePath = d.path;
  // LRM marks keep the leading "/" in place in the right-to-left box that
  // cuts long paths at the start.
  $('browse-cur').textContent = '\u200e' + d.path + '\u200e';
  $('browse-cur').title = d.path;
  // Windows' list of drives is not a folder to pick or make one in
  $('browse-use').disabled = $('browse-new').disabled = !!d.virtual;

  const list = $('browse-list');
  list.innerHTML = '';
  if (d.parent) list.appendChild(browseItem('↑', '..', () => browseTo(d.parent)));
  d.dirs.forEach((name, i) =>
    list.appendChild(browseItem('▸', name, () => browseTo(d.dir_paths[i]))));
  (d.files || []).forEach((name, i) =>
    list.appendChild(browseItem('◆', name, () => openRaw(d.file_paths[i], browseError))));
  if (!d.parent && !d.dirs.length && !(d.files || []).length) {
    const e = document.createElement('div');
    e.className = 'browse-empty';
    e.textContent = '(no subfolders)';
    list.appendChild(e);
  }
}

const browseError = text => setMsg($('browse-msg'), text, 'err');

// Open a raw where it is on the host and process it in place; its export
// saves the frame positions beside it (<raw>.crops.json) for next time.
async function openRaw(path, onError) {
  const resp = await postJSON('/api/open', { path });
  const d = await resp.json();
  if (!resp.ok) { onError(d.error || 'could not open'); return; }
  showRaw(d.path, d.dx);
  if (d.film) $('proc-film').value = d.film;
  closeBrowser();
  showSpace('process');   // watch the progress there; Frames opens when done
  processRaw();
}

async function useWorkDir(path, onError) {
  const resp = await postJSON('/api/workdir', { path });
  const d = await resp.json();
  if (!resp.ok) { onError(d.error || 'could not set the work folder'); return false; }
  showWorkDir(d.work_dir, d.next_order);
  return true;
}

$('scan-out-btn').addEventListener('click', () => pick(false));
$('raw-open').addEventListener('click', () => pick(true));
$('home-raw').addEventListener('click', () => pick(true));

$('browse-close').addEventListener('click', closeBrowser);
$('browse-cancel').addEventListener('click', closeBrowser);
$('browse-modal').addEventListener('click', e => {
  if (e.target === $('browse-modal')) closeBrowser();
});

$('browse-use').addEventListener('click', async () => {
  if (!browsePath) return;
  if (await useWorkDir(browsePath, browseError)) closeBrowser();
});

$('browse-new').addEventListener('click', async () => {
  const name = prompt('New folder name:');
  if (!name) return;
  const resp = await postJSON('/api/mkdir', { path: browsePath, name });
  const d = await resp.json();
  if (!resp.ok) { browseError(d.error || 'mkdir failed'); return; }
  browseTo(d.path);   // descend into the freshly created folder
});
