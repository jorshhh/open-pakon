<script setup lang="ts">
// The house button: a hard shadow that lifts on hover and presses on click.
// primary (amber), ghost (white), dark (on the amber banner), stop (red).
withDefaults(defineProps<{ variant?: 'primary' | 'ghost' | 'dark' | 'stop'; icon?: boolean }>(), {
  variant: 'primary',
  icon: false,
});
const look = {
  primary: 'bg-accent text-ink',
  ghost: 'bg-surface text-ink enabled:hover:bg-paper',
  dark: 'bg-ink text-accent',
  stop: 'bg-surface text-danger border-danger! shadow-[2px_2px_0_0_var(--color-danger)]',
};
</script>

<template>
  <button
    :class="[
      'cursor-pointer border-2 border-ink font-mono text-body font-semibold uppercase shadow-hard-sm transition-all duration-150',
      'enabled:hover:-translate-0.5 enabled:hover:shadow-hard enabled:active:translate-0.5 enabled:active:shadow-none',
      'disabled:cursor-not-allowed disabled:opacity-50 disabled:shadow-none',
      icon ? 'grid size-9 shrink-0 place-items-center p-0 [&_svg]:size-4.5' : 'px-4 py-2',
      look[variant],
    ]"
  ><slot /></button>
</template>
