// Global live state: status stream, device config and daemon info.

import { get, setUnauthorizedHandler, type DeviceConfig, type Info, type Status } from './api'

export interface LogLine {
  time: number
  msg: string
}

export interface PidInternals {
  b: number
  p: number
  i: number
  d: number
  t: number
}

export const live = $state({
  status: null as Status | null,
  config: {} as DeviceConfig,
  info: null as Info | null,
  streamOk: false,
  log: [] as LogLine[],
  pidint: null as PidInternals | null,
  /** Incremented on each new status, used by charts to append points */
  tick: 0,
  loginOpen: false,
})

export const units = () => (live.config.u as string) || 'F'

let es: EventSource | null = null
let retry = 0

function connect() {
  es?.close()
  es = new EventSource('/api/v2/events')
  es.addEventListener('open', () => {
    live.streamOk = true
    retry = 0
  })
  es.addEventListener('error', () => {
    live.streamOk = false
    if (es?.readyState === EventSource.CLOSED) {
      es = null
      setTimeout(connect, Math.min(30000, 1000 * 2 ** retry++))
    }
  })
  es.addEventListener('hmstatus', (e) => {
    const s = JSON.parse((e as MessageEvent).data) as Status
    const prev = live.status
    live.status = s
    live.tick++
    // refresh config when names change or after (re)connect
    if (!prev || prev.temps.some((p, i) => p.n !== s.temps[i]?.n)) void refreshConfig()
  })
  es.addEventListener('alarm', () => {
    void refreshConfig()
  })
  es.addEventListener('log', (e) => {
    const { msg } = JSON.parse((e as MessageEvent).data)
    live.log = [...live.log.slice(-49), { time: Date.now() / 1000, msg }]
  })
  es.addEventListener('pidint', (e) => {
    live.pidint = JSON.parse((e as MessageEvent).data)
  })
}

export async function refreshConfig() {
  try {
    live.config = await get<DeviceConfig>('/config')
  } catch {
    /* ignore */
  }
}

export async function refreshInfo() {
  try {
    live.info = await get<Info>('/info')
  } catch {
    /* ignore */
  }
}

export function startLive() {
  setUnauthorizedHandler(() => {
    live.loginOpen = true
    void refreshInfo()
  })
  connect()
  void refreshConfig()
  void refreshInfo()
  setInterval(refreshInfo, 15000)
  // Re-sync config periodically (another client may change it)
  setInterval(refreshConfig, 30000)
}
