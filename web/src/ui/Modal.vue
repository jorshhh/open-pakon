<script setup lang="ts">
// A dialog over a dark backdrop; a click outside closes it. `top` stacks
// it above another dialog (the ask box over the folder browser).
defineProps<{ title?: string; small?: boolean; top?: boolean; as?: string }>();
const emit = defineEmits<{ close: [] }>();
</script>

<template>
  <div :class="['fixed inset-0 flex items-center justify-center bg-black/80', top ? 'z-[101]' : 'z-[100]']" @click.self="emit('close')">
    <component :is="as || 'div'" role="dialog" :aria-label="title"
               :class="['flex max-h-[80vh] flex-col overflow-hidden border-2 border-ink bg-surface shadow-modal', small ? 'w-[min(420px,92vw)]' : 'w-[min(560px,92vw)]']">
      <div v-if="title" class="flex items-center justify-between border-b-2 border-ink bg-accent px-5 py-4">
        <h2 class="font-display text-heading font-bold uppercase">{{ title }}</h2>
        <button class="cursor-pointer border-2 border-ink bg-surface px-2.5 py-1 text-heading leading-none" aria-label="Close" @click="emit('close')">✕</button>
      </div>
      <slot />
      <div class="flex items-center gap-2.5 border-t-2 border-ink bg-surface px-5 py-3.5">
        <slot name="foot" />
      </div>
    </component>
  </div>
</template>
