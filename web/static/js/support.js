// Settings › Troubleshooting: the log folder and a bug report by email.
import { $, setMsg } from './util.js';

// Show log only where the service can open the log folder (/api/status `logs`).
export function showLogs(on) { $('logs-btn').classList.toggle('hidden', !on); }

$('logs-btn').addEventListener('click', () => fetch('/api/logs/open', { method: 'POST' }));

// The email is opened by the service (the app window cannot follow a
// mailto: link); in a browser, the page opens the link itself.
$('report-btn').addEventListener('click', async () => {
  setMsg($('report-msg'), '');
  try {
    const d = await fetch('/api/report', { method: 'POST' }).then(r => r.json());
    if (!d.opened) window.location.href = d.url;
    setMsg($('report-msg'), 'Opening your mail app… attach pakon.log from Show log.', 'ok');
  } catch {
    setMsg($('report-msg'), 'Could not open the mail app. Write to jorsh@jor.sh.', 'err');
  }
});
