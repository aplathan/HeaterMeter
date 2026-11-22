// Formatting helpers.

export function fmtTemp(v: number | null | undefined, digits = 1): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '–'
  return v.toFixed(digits)
}

export function unitSymbol(u: unknown): string {
  if (u === 'C') return '°C'
  if (u === 'R') return 'Ω'
  if (u === 'A') return 'ADC'
  return '°F'
}

export function fmtTime(ts: number): string {
  return new Date(ts * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
}

export function fmtDateTime(ts: number): string {
  return new Date(ts * 1000).toLocaleString([], {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

export function fmtDuration(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) return '–'
  const h = Math.floor(secs / 3600)
  const m = Math.floor((secs % 3600) / 60)
  if (h > 24) return `${Math.floor(h / 24)} d ${h % 24} h`
  if (h > 0) return `${h} h ${m.toString().padStart(2, '0')} min`
  return `${m} min`
}

/** Estimated time (unix seconds) a probe rising at `dph` reaches `target`. */
export function eta(current: number | null, target: number | null, dph: number | undefined, now: number): number | null {
  if (current === null || target === null || !dph || dph <= 0.5 || target <= current) return null
  const hours = (target - current) / dph
  if (hours > 48) return null
  return now + hours * 3600
}

/** Number parse helper for config values. */
export function num(v: unknown, fallback = 0): number {
  const n = typeof v === 'number' ? v : parseFloat(String(v ?? ''))
  return Number.isFinite(n) ? n : fallback
}
