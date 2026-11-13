// Spaces: one page per step, switched from the bar along the bottom.
import { $ } from './util.js';

export let currentSpace = null;
const listeners = [];

// fn(name, previous) runs on every showSpace, after the switch.
export function onShow(fn) { listeners.push(fn); }

export function showSpace(name) {
  const prev = currentSpace;
  currentSpace = name;
  document.querySelectorAll('.space').forEach(s =>
    s.classList.toggle('active', s.id === 'space-' + name));
  document.querySelectorAll('.spaces button').forEach(b =>
    b.setAttribute('aria-selected', String(b.dataset.space === name)));
  for (const fn of listeners) fn(name, prev);
}

// The red dot on a space's tab while work runs there.
export const setBusy = (name, on) => $('tab-' + name).classList.toggle('busy', on);

// The body keeps room for the bar along the bottom: its height and a gap.
export function fitBar() {
  document.body.style.paddingBottom =
    (document.querySelector('.spaces').offsetHeight + 16) + 'px';
}

document.querySelectorAll('.spaces button').forEach(b =>
  b.addEventListener('click', () => showSpace(b.dataset.space)));
window.addEventListener('resize', fitBar);
