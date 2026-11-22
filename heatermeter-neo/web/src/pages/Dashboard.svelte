<script lang="ts">
  import { get, type History } from '../lib/api'
  import { live, units } from '../lib/live.svelte'
  import { unitSymbol } from '../lib/format'
  import { t } from '../lib/i18n.svelte'
  import PitCard from '../components/PitCard.svelte'
  import ProbeCard from '../components/ProbeCard.svelte'
  import Chart from '../components/Chart.svelte'

  const COLORS = ['var(--c-t0)', 'var(--c-t1)', 'var(--c-t2)', 'var(--c-t3)']
  const RANGES: [string, number][] = [
    ['range_1h', 3600],
    ['range_3h', 3 * 3600],
    ['range_6h', 6 * 3600],
    ['range_12h', 12 * 3600],
    ['range_all', 0],
  ]

  let range = $state(loadRange())
  let history = $state<History | null>(null)
  let loadedSession = $state<number | null | undefined>(undefined)

  function loadRange(): number {
    try {
      const v = parseInt(localStorage.getItem('range') ?? '')
      if (RANGES.some(([, s]) => s === v)) return v
    } catch {
      /* ignore */
    }
    return 3 * 3600
  }

  function setRange(v: number) {
    range = v
    try {
      localStorage.setItem('range', String(v))
    } catch {
      /* ignore */
    }
  }

  async function load() {
    try {
      history = await get<History>('/sessions/current/history?points=2500')
    } catch {
      history = { step: 1, time: [], sp: [], t0: [], t1: [], t2: [], t3: [], out: [], fan: [], servo: [] }
    }
  }

  // Reload history when the current session changes (new cook, reconnect)
  $effect(() => {
    const s = live.info?.session
    if (s !== loadedSession) {
      loadedSession = s
      void load()
    }
  })

  const unit = $derived(unitSymbol(units()))
  const status = $derived(live.status)
  const names = $derived((status?.temps ?? []).map((p) => p.n))
  const now = $derived(status?.time ?? Date.now() / 1000)
  const food = $derived((status?.temps ?? []).slice(1).map((p, i) => ({ p, i: i + 1 })))
</script>

<div class="layout">
  <div class="top">
    <PitCard {status} {unit} lidtrack={live.info?.lidtrack_enabled ?? false} />
    <div class="probes">
      {#each food as { p, i } (i)}
        <ProbeCard probe={p} index={i} {unit} {now} color={COLORS[i]} />
      {/each}
    </div>
  </div>

  <section class="card">
    <div class="card-head">
      <h2>{t('history')}</h2>
      <div class="seg">
        {#each RANGES as [k, s] (k)}
          <button class="small" class:sel={range === s} onclick={() => setRange(s)}>{t(k as 'range_1h')}</button>
        {/each}
      </div>
    </div>
    <Chart {history} {names} {unit} {status} tick={live.tick} window={range} />
  </section>

  {#if live.pidint}
    <section class="card">
      <div class="card-head"><h2>{t('pidint')}</h2></div>
      <div class="row num muted">
        B {live.pidint.b} · P {live.pidint.p} · I {live.pidint.i} · D {live.pidint.d} · ΔT {live.pidint.t}
      </div>
    </section>
  {/if}
</div>

<style>
  .layout {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .top {
    display: grid;
    grid-template-columns: minmax(320px, 1.1fr) 2fr;
    gap: 16px;
  }
  .probes {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
    gap: 16px;
  }
  .seg {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .seg .sel {
    background: var(--accent);
    color: var(--accent-text);
    border-color: transparent;
  }
  @media (max-width: 900px) {
    .top {
      grid-template-columns: 1fr;
    }
  }
</style>
