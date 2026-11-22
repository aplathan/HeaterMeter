// Daemon-side settings (alarms, notifications, ramp, api...) shared by the settings pages.

import { get, send, type Settings } from './api'
import { live } from './live.svelte'

export const daemon = $state({
  settings: null as Settings | null,
  error: '',
})

export async function loadSettings() {
  try {
    const s = await get<Settings>('/settings')
    // Make sure every alarm has an action entry so forms can bind to it
    for (const p of [0, 1, 2, 3])
      for (const k of ['L', 'H'])
        s.alarms.actions[`${p}${k}`] ??= { email: false, sms: false, push: false, setpoint: null, action: 'ring' }
    daemon.settings = s
    daemon.error = ''
  } catch (e) {
    daemon.error = (e as Error).message
  }
}

export async function saveSettings() {
  if (!daemon.settings) return
  await send('PUT', '/settings', daemon.settings)
  await loadSettings()
}

/** Feedback helper: run an async action and expose a transient message. */
export function action() {
  const st = $state({ busy: false, msg: '', ok: true })
  return {
    get busy() {
      return st.busy
    },
    get msg() {
      return st.msg
    },
    get ok() {
      return st.ok
    },
    async run(f: () => Promise<string | void>) {
      st.busy = true
      st.msg = ''
      try {
        const r = await f()
        st.ok = true
        st.msg = r ?? '✓'
      } catch (e) {
        st.ok = false
        st.msg = (e as Error).message
      } finally {
        st.busy = false
        const m = st.msg
        setTimeout(() => {
          if (st.msg === m) st.msg = ''
        }, 6000)
      }
    },
  }
}

export const cfg = (k: string): string => {
  const v = live.config[k]
  return v === null || v === undefined ? '' : String(v)
}
