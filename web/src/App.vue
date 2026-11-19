<script setup lang="ts">
import { app } from './state/app';
import { badgeText, scannerState } from './status';
import AdjustSpace from './components/AdjustSpace.vue';
import AskDialog from './components/AskDialog.vue';
import BrowseModal from './components/BrowseModal.vue';
import ConsentPop from './components/ConsentPop.vue';
import ExportSpace from './components/ExportSpace.vue';
import FramesSpace from './components/FramesSpace.vue';
import HomeSpace from './components/HomeSpace.vue';
import ProcessSpace from './components/ProcessSpace.vue';
import ScanSpace from './components/ScanSpace.vue';
import SettingsSpace from './components/SettingsSpace.vue';
import SpaceBar from './components/SpaceBar.vue';
import UpdateBar from './components/UpdateBar.vue';
import Dot from './ui/Dot.vue';

// One page per step, switched from the bar along the bottom. Spaces stay
// mounted (v-show), so a job keeps running while another space is shown.
const spaces = [
  { id: 'home', is: HomeSpace },
  { id: 'scan', is: ScanSpace },
  { id: 'process', is: ProcessSpace },
  { id: 'frames', is: FramesSpace },
  { id: 'adjust', is: AdjustSpace },
  { id: 'export', is: ExportSpace },
  { id: 'settings', is: SettingsSpace },
] as const;
</script>

<template>
  <header class="mb-4 flex items-center gap-4 border-b-2 border-ink pb-2">
    <h1 class="font-display text-title leading-none font-bold tracking-tight uppercase">Pakon</h1>
    <div :title="app.status?.state_error || ''"
         :class="['flex items-center gap-1.5 border-2 border-ink bg-surface px-2 py-0.5 font-mono text-small font-semibold uppercase shadow-hard-sm',
                  scannerState === 'warm' || scannerState === 'cold' ? 'text-ink' : 'text-muted']">
      <Dot :state="scannerState" /><span>{{ badgeText }}</span>
    </div>
  </header>

  <UpdateBar />

  <section v-for="s in spaces" v-show="app.space === s.id" :key="s.id" :id="'space-' + s.id"
           role="tabpanel" :aria-labelledby="'tab-' + s.id">
    <component :is="s.is" />
  </section>

  <ConsentPop />
  <SpaceBar />
  <BrowseModal />
  <AskDialog />
</template>
