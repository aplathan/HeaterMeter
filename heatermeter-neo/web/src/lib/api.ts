// Types and HTTP helpers for the linkmeterd /api/v2 API.

export interface ProbeStatus {
  n: string
  c: number | null
  dph?: number
  rf?: { s: number; b: number } | 0
  a: { l: number | null; h: number | null; r: 'L' | 'H' | null }
}

export interface Status {
  time: number
  set: number | null
  lid: number
  fan: { c: number | null; a: number | null; f: number | null; s: number | null }
  adc?: (number | null)[]
  ramp?: { s: number; ta: number; tr: number }
  temps: ProbeStatus[]
}

export type DeviceConfig = Record<string, string | number | null>

export interface RampParams {
  enabled: boolean
  watch: number[]
  trigger: number
  target: number
  start_setpoint: number | null
}

export interface Info {
  version: string
  ucid: string | null
  connected: boolean
  configured: boolean
  port: string | null
  hostname: string
  ip: string | null
  uptime: number | null
  daemon_uptime: number
  cpu_temp: number | null
  admin: boolean
  password_set: boolean
  guest_read: boolean
  simulator: boolean
  firmware_enabled: boolean
  wifi_enabled: boolean
  lidtrack_enabled: boolean
  ramp: RampParams
  session: number | null
  time: number
}

export interface History {
  step: number
  time: number[]
  sp: (number | null)[]
  t0: (number | null)[]
  t1: (number | null)[]
  t2: (number | null)[]
  t3: (number | null)[]
  out: (number | null)[]
  fan: (number | null)[]
  servo: (number | null)[]
}

export interface Session {
  id: number
  name: string
  started: number
  last: number
  samples: number
  current: boolean
}

export type RingAction = 'ring' | 'silence' | 'disable'
export interface AlarmAction {
  email: boolean
  sms: boolean
  push: boolean
  setpoint: string | null
  action: RingAction
}

export interface Settings {
  lidtrack_enabled: boolean
  guest_read: boolean
  api: { disabled: boolean; allowcors: boolean; key: string }
  ramp: RampParams
  alarms: {
    actions: Record<string, AlarmAction>
    email: { to_name: string; to_address: string; from_name: string; subject: string; body: string }
    sms: { to_address: string; message: string }
    push: {
      provider: 'none' | 'pushover' | 'pushbullet' | 'ntfy' | 'webhook'
      title: string
      message: string
      pushbullet_key: string
      pushover_user: string
      pushover_token: string
      pushover_sound: string
      ntfy_url: string
      ntfy_token: string
      webhook_url: string
    }
    scripts_dir: string
  }
  smtp: {
    host: string
    port: number
    security: 'starttls' | 'tls' | 'none'
    username: string
    password: string
    from_address: string
  }
  ui: { user_css: string }
  wifi: { enabled: boolean; interface: string; fallback_ap: boolean; ap_ssid: string; ap_password: string }
}

export interface WifiNetwork {
  ssid: string
  signal: number
  security: string
  channel: string
  in_use: boolean
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message)
  }
}

async function handle<T>(r: Response): Promise<T> {
  const ct = r.headers.get('content-type') ?? ''
  const body = ct.includes('json') ? await r.json() : await r.text()
  if (!r.ok) {
    const msg = typeof body === 'object' && body ? (body.error ?? body.message ?? r.statusText) : String(body)
    if (r.status === 401) onUnauthorized?.()
    throw new ApiError(r.status, msg)
  }
  return body as T
}

/** Called when the server says we need to log in. */
export let onUnauthorized: (() => void) | null = null
export function setUnauthorizedHandler(f: () => void) {
  onUnauthorized = f
}

export function get<T>(path: string): Promise<T> {
  return fetch(`/api/v2${path}`, { credentials: 'same-origin' }).then((r) => handle<T>(r))
}

export function send<T>(method: string, path: string, body?: unknown): Promise<T> {
  return fetch(`/api/v2${path}`, {
    method,
    credentials: 'same-origin',
    headers: body === undefined ? {} : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  }).then((r) => handle<T>(r))
}

export const post = <T>(path: string, body?: unknown) => send<T>('POST', path, body)

/** Write HeaterMeter settings (`/set?k=v` for each entry). */
export function setValues(vals: Record<string, string | number>) {
  const body: Record<string, string> = {}
  for (const [k, v] of Object.entries(vals)) body[k] = String(v)
  return post<{ ok: boolean; log: string }>('/set', body)
}

/** POST a multipart form and stream the text/plain response line by line. */
export async function streamLines(path: string, form: FormData, onLine: (l: string) => void): Promise<void> {
  const r = await fetch(`/api/v2${path}`, { method: 'POST', body: form, credentials: 'same-origin' })
  if (!r.ok || !r.body) {
    onLine(await r.text())
    return
  }
  const reader = r.body.getReader()
  const dec = new TextDecoder()
  let buf = ''
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    buf += dec.decode(value, { stream: true })
    let i: number
    while ((i = buf.indexOf('\n')) >= 0) {
      onLine(buf.slice(0, i))
      buf = buf.slice(i + 1)
    }
  }
  if (buf) onLine(buf)
}
