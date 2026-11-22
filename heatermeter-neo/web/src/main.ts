import './app.css'
import { mount } from 'svelte'
import App from './App.svelte'
import { startLive } from './lib/live.svelte'

try {
  const theme = localStorage.getItem('theme')
  if (theme === 'light' || theme === 'dark') document.documentElement.dataset.theme = theme
} catch {
  /* storage unavailable */
}

startLive()
mount(App, { target: document.getElementById('app')! })
