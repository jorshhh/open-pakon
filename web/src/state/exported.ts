// What Export shows after an export.
import { reactive } from 'vue';
import { getJSON } from '../api';

export const exported = reactive({
  frames: [] as { index: number }[],
  folder: '',
  reveal: '',          // the service's name for "open the folder" ('' = cannot)
  stamp: 0,
});

export function resetExport() {
  exported.frames = [];
}

// The export's frames, for Export.
export async function loadFrames() {
  const d = await getJSON<{ folder: string; frames: { index: number }[]; reveal?: string }>('/api/frames');
  exported.reveal = d.reveal || '';
  exported.folder = d.folder || '';
  exported.stamp = Date.now();
  exported.frames = d.frames;
}
