// The page: Vue, styled with Tailwind, built by Vite into web/static.
import { createApp } from 'vue';
import App from './App.vue';
import { startPolling } from './state/app';
import { reportErrors } from './telemetry';
import './main.css';

reportErrors();      // first, so it sees any error
createApp(App).mount('#app');
startPolling();
