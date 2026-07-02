import { mount } from 'svelte';
import { invoke } from '@tauri-apps/api/core';
import './styles/global.css';
import App from './App.svelte';

const app = mount(App, {
  target: document.getElementById('app')!,
});

// Native feel: suppress the WebKit context menu everywhere except inside
// editable fields, where the native cut/copy/paste menu is genuinely useful.
document.addEventListener('contextmenu', (e) => {
  const el = e.target instanceof Element ? e.target : null;
  if (el?.closest('input, textarea, [contenteditable]:not([contenteditable="false"])')) return;
  e.preventDefault();
});

// Cold-launch white-flash fix: the window is created hidden (tauri.conf.json
// "visible": false) and revealed only after the first real paint (double rAF
// = layout done + a frame painted). Rust decides whether to actually show —
// a `--hidden` autostart-into-tray launch stays hidden.
requestAnimationFrame(() => {
  requestAnimationFrame(() => {
    invoke('show_when_ready').catch(() => {
      /* Not in a Tauri context (vite dev in a plain browser) — nothing to show. */
    });
  });
});

export default app;
