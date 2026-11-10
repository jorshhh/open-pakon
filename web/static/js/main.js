// Entry point: each module wires its own space when imported; this starts
// the page. No framework and no build step: the browser loads these
// ES modules as they are.
import { $ } from './util.js';
import { showSpace, fitBar } from './spaces.js';
import './telemetry.js';   // first, so it sees the first page and any error
import './update.js';
import './scan.js';
import './process.js';
import './frames.js';
import './export.js';
import './browse.js';
import { startPolling } from './status.js';

$('home-scan').addEventListener('click', () => showSpace('scan'));

showSpace('home');
fitBar();
startPolling();
