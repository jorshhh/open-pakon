<script setup lang="ts">
// The page's folder browser (/api/browse), for a plain browser: picks the
// work folder, or a .raw to open. The app window uses the system's dialog.
import { ref, watch } from 'vue';
import { getJSON, postJSON } from '../api';
import { ask } from '../ask';
import { app, browse, useWorkDir } from '../state/app';
import { openRaw } from '../state/roll';
import Btn from '../ui/Btn.vue';
import Modal from '../ui/Modal.vue';

interface Listing {
  path: string; parent?: string | null; virtual?: boolean;
  dirs: string[]; dir_paths: string[]; files?: string[]; file_paths?: string[];
}

const list = ref<Listing | null>(null);
const error = ref('');

async function go(path?: string | null) {
  error.value = '';
  const q = new URLSearchParams();
  if (path) q.set('path', path);
  if (browse.raws) q.set('raws', 'true');
  list.value = await getJSON<Listing>('/api/browse?' + q);
}

watch(() => browse.open, open => { if (open) go(app.status?.work_dir); else list.value = null; });

const close = () => { browse.open = false; };

async function useThis() {
  if (!list.value) return;
  const err = await useWorkDir(list.value.path);
  if (err) error.value = err; else close();
}

async function openFile(path: string) {
  const err = await openRaw(path);
  if (err) error.value = err;
}

async function newFolder() {
  if (!list.value) return;
  const name = await ask({ label: 'New folder name', ok: 'Create' });
  if (!name) return;
  const resp = await postJSON('/api/mkdir', { path: list.value.path, name });
  const d = await resp.json();
  if (!resp.ok) { error.value = d.error || 'mkdir failed'; return; }
  go(d.path);   // into the new folder
}

const item = 'flex cursor-pointer items-center gap-2.5 border-b border-ink px-5 py-2 font-mono text-body hover:bg-surface hover:font-semibold';
</script>

<template>
  <Modal v-if="browse.open" :title="browse.raws ? 'Open a .raw' : 'Choose work folder'" @close="close">
    <!-- right-to-left so a long path is cut at its start; the LRM marks keep
         the leading "/" in place -->
    <div class="truncate border-b-2 border-ink bg-surface-2 px-5 py-2.5 text-left font-mono text-small font-semibold [direction:rtl]"
         :title="list?.path">{{ list ? '‎' + list.path + '‎' : '' }}</div>
    <div class="h-[min(360px,50vh)] overflow-y-auto bg-paper">
      <template v-if="list">
        <div v-if="list.parent" :class="item" @click="go(list.parent)"><span class="shrink-0 font-bold text-danger">↑</span>..</div>
        <div v-for="(name, i) in list.dirs" :key="'d' + name" :class="item" @click="go(list.dir_paths[i])">
          <span class="shrink-0 font-bold text-danger">▸</span>{{ name }}
        </div>
        <div v-for="(name, i) in list.files ?? []" :key="'f' + name" :class="item" @click="openFile(list.file_paths![i])">
          <span class="shrink-0 font-bold text-danger">◆</span>{{ name }}
        </div>
        <div v-if="!list.parent && !list.dirs.length && !(list.files ?? []).length"
             class="px-5 py-4 font-mono text-body text-muted">(no subfolders)</div>
      </template>
    </div>
    <template #foot>
      <!-- Windows' list of drives is not a folder to pick or make one in -->
      <Btn v-if="!browse.raws" variant="ghost" :disabled="!!list?.virtual" @click="newFolder">New folder…</Btn>
      <span class="flex-1"></span>
      <span class="font-mono text-body font-semibold text-danger">{{ error }}</span>
      <Btn variant="ghost" @click="close">Cancel</Btn>
      <Btn v-if="!browse.raws" :disabled="!!list?.virtual" @click="useThis">Use this folder</Btn>
    </template>
  </Modal>
</template>
