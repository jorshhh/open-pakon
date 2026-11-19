<script setup lang="ts">
// A card: a heavy rule on top, a hard shadow. `title` with an optional
// #info (an InfoTip beside it) and #tools (right of a ruled header).
defineProps<{ title?: string; ruled?: boolean }>();
</script>

<template>
  <div class="relative mb-6 border-2 border-ink bg-surface p-6 shadow-hard before:absolute before:inset-x-0 before:top-0 before:h-1 before:bg-ink last:mb-0">
    <div v-if="title || $slots.title"
         :class="ruled ? 'mb-5 flex items-center justify-between gap-3 border-b-2 border-ink pb-2.5' : 'mb-4 flex items-center gap-2.5'">
      <div class="flex items-center gap-2.5">
        <h2 class="font-display text-title font-bold uppercase"><slot name="title">{{ title }}</slot></h2>
        <slot name="info" />
      </div>
      <slot name="tools" />
    </div>
    <slot />
  </div>
</template>
