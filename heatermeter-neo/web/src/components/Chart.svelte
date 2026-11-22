<script lang="ts">
  import uPlot from 'uplot'
  import 'uplot/dist/uPlot.min.css'
  import type { History, Status } from '../lib/api'
  import { t } from '../lib/i18n.svelte'

  let {
    history,
    names = ['Pit', 'Food 1', 'Food 2', 'Ambient'],
    unit = '°F',
    status = null,
    tick = 0,
    window: windowSecs = 0,
    height = 340,
  }: {
    history: History | null
    names?: string[]
    unit?: string
    /** Latest live status; appended when `tick` changes */
    status?: Status | null
    tick?: number
    /** Visible time window in seconds (0 = everything) */
    window?: number
    height?: number
  } = $props()

  let el: HTMLDivElement | undefined = $state()
  let plot: uPlot | null = null
  let data: (number | null)[][] = [[], [], [], [], [], [], [], []]
  let themeKey = $state(0)

  function css(name: string) {
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  }

  function toData(h: History | null): (number | null)[][] {
    if (!h) return [[], [], [], [], [], [], [], []]
    const out: (number | null)[] = []
    const lid: (number | null)[] = []
    for (const v of h.out) {
      out.push(v === null ? null : v < 0 ? 0 : v)
      lid.push(v !== null && v < 0 ? 100 : null)
    }
    return [h.time, h.sp, h.t0, h.t1, h.t2, h.t3, out, lid]
  }

  function opts(width: number): uPlot.Options {
    const grid = { stroke: css('--grid'), width: 1 }
    const axis = { stroke: css('--muted'), grid, ticks: { stroke: css('--grid'), width: 1 }, font: '12px ' + css('--font') }
    const temp = (c: string, label: string): uPlot.Series => ({
      label,
      stroke: css(c),
      width: 2,
      spanGaps: false,
      value: (_u, v) => (v == null ? '–' : `${v.toFixed(1)}${unit}`),
    })
    return {
      width,
      height,
      cursor: { drag: { x: true, y: false }, points: { size: 7 } },
      hooks: { setCursor: [() => legendLatest()] },
      scales: {
        x: { time: true },
        y: { auto: true, range: (_u, min, max) => [Math.floor((min ?? 0) / 25) * 25, Math.ceil(((max ?? 100) + 5) / 25) * 25] },
        pct: { range: [0, 100] },
      },
      legend: { live: true },
      series: [
        { value: (_u, v) => (v == null ? '' : new Date(v * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })) },
        {
          label: t('setpoint'),
          stroke: css('--c-sp'),
          width: 1.5,
          dash: [6, 4],
          paths: uPlot.paths.stepped!({ align: 1 }),
          value: (_u, v) => (v == null ? '–' : `${Math.round(v)}${unit}`),
        },
        temp('--c-t0', names[0]),
        temp('--c-t1', names[1]),
        temp('--c-t2', names[2]),
        temp('--c-t3', names[3]),
        {
          label: t('output'),
          scale: 'pct',
          stroke: css('--c-out'),
          fill: css('--c-out-fill'),
          width: 1,
          value: (_u, v) => (v == null ? '–' : `${Math.round(v)} %`),
        },
        {
          label: t('lid_open'),
          scale: 'pct',
          stroke: 'transparent',
          fill: css('--c-lid'),
          width: 0,
          paths: uPlot.paths.stepped!({ align: 1 }),
          value: (_u, v) => (v ? '●' : ''),
        },
      ],
      axes: [
        { ...axis },
        { ...axis, size: 52, values: (_u, s) => s.map((v) => `${v}°`) },
        { ...axis, scale: 'pct', side: 1, size: 44, grid: { show: false }, values: (_u, s) => s.map((v) => `${v}%`) },
      ],
    }
  }

  /** Show the latest values in the legend while the cursor is not on the plot. */
  function legendLatest() {
    if (!plot) return
    const n = (data[0] as number[]).length
    if (n && (plot.cursor.left == null || plot.cursor.left < 0)) plot.setLegend({ idx: n - 1 })
  }

  function applyWindow() {
    if (!plot) return
    legendLatest()
    const xs = data[0] as number[]
    if (!xs.length) return
    const last = xs[xs.length - 1]
    const first = xs[0]
    const min = windowSecs > 0 ? Math.max(first, last - windowSecs) : first
    plot.setScale('x', { min, max: last })
  }

  function build() {
    if (!el) return
    plot?.destroy()
    plot = new uPlot(opts(el.clientWidth || 600), data as uPlot.AlignedData, el)
    applyWindow()
  }

  // (Re)build when the element, history, names, unit or theme change
  $effect(() => {
    void themeKey
    void names.join('|')
    void unit
    data = toData(history)
    if (el) build()
    return () => {
      plot?.destroy()
      plot = null
    }
  })

  $effect(() => {
    void windowSecs
    applyWindow()
  })

  // Live append
  $effect(() => {
    void tick
    const s = status
    if (!plot || !s || !history) return
    const xs = data[0] as number[]
    if (xs.length && s.time <= xs[xs.length - 1]) return
    const sp = typeof s.set === 'number' && s.set > 0 ? s.set : null
    const row = [s.time, sp, s.temps[0]?.c ?? null, s.temps[1]?.c ?? null, s.temps[2]?.c ?? null, s.temps[3]?.c ?? null,
      s.lid > 0 ? 0 : s.fan.c, s.lid > 0 ? 100 : null]
    row.forEach((v, i) => data[i].push(v))
    plot.setData(data as uPlot.AlignedData, false)
    applyWindow()
  })

  $effect(() => {
    if (!el) return
    const ro = new ResizeObserver(() => plot?.setSize({ width: el!.clientWidth, height }))
    ro.observe(el)
    const mq = matchMedia('(prefers-color-scheme: dark)')
    const bump = () => themeKey++
    mq.addEventListener('change', bump)
    const mo = new MutationObserver(bump)
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })
    return () => {
      ro.disconnect()
      mq.removeEventListener('change', bump)
      mo.disconnect()
    }
  })
</script>

<div class="chart" bind:this={el}></div>
{#if history && history.time.length === 0}
  <div class="empty muted">{t('no_data')}</div>
{/if}

<style>
  .chart {
    width: 100%;
    min-height: 200px;
  }
  .empty {
    text-align: center;
    padding: 8px;
  }
  .chart :global(.u-legend) {
    font-size: 0.82rem;
    color: var(--muted);
    margin-top: 6px;
  }
  .chart :global(.u-legend .u-series) {
    padding: 2px 6px;
    cursor: pointer;
  }
  .chart :global(.u-legend .u-value) {
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .chart :global(.u-legend .u-off) {
    opacity: 0.35;
  }
  .chart :global(.u-select) {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
  }
</style>
