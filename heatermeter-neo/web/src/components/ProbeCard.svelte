<script lang="ts">
  import type { ProbeStatus } from '../lib/api'
  import { setValues } from '../lib/api'
  import { eta, fmtTemp, fmtTime } from '../lib/format'
  import { t } from '../lib/i18n.svelte'
  import Modal from './Modal.svelte'

  let {
    probe,
    index,
    unit,
    now,
    color,
  }: { probe: ProbeStatus; index: number; unit: string; now: number; color: string } = $props()

  const lo = $derived(probe.a.l)
  const hi = $derived(probe.a.h)
  const loOn = $derived(lo !== null && lo > 0)
  const hiOn = $derived(hi !== null && hi > 0)
  const done = $derived(hiOn ? eta(probe.c, hi, probe.dph, now) : null)
  const rf = $derived(probe.rf)

  let open = $state(false)
  let eLo = $state('')
  let eHi = $state('')
  let eLoOn = $state(false)
  let eHiOn = $state(false)

  function edit() {
    eLo = lo !== null && lo !== 0 ? String(Math.abs(lo)) : ''
    eHi = hi !== null && hi !== 0 ? String(Math.abs(hi)) : ''
    eLoOn = loOn
    eHiOn = hiOn
    open = true
  }

  function enc(v: string, on: boolean): string {
    const n = Math.abs(parseInt(v))
    if (!Number.isFinite(n) || n === 0) return ''
    return String(on ? n : -n)
  }

  async function save() {
    const prefix = ','.repeat(index * 2)
    await setValues({ al: `${prefix}${enc(eLo, eLoOn)},${enc(eHi, eHiOn)}` })
    open = false
  }
</script>

<section class="card probe" class:ringing={probe.a.r} style:--pc={color}>
  <div class="head">
    <span class="dot"></span>
    <h3>{probe.n}</h3>
    {#if rf !== undefined}
      <span class="chip" title="RF">{rf === 0 ? 'RF ✕' : `RF ${rf.s}${rf.b ? ' 🔋' : ''}`}</span>
    {/if}
  </div>
  <div class="temp num" class:unplugged={probe.c == null}>
    {#if probe.c == null}
      <span class="na">{t('unplugged')}</span>
    {:else}
      {fmtTemp(probe.c)}<span class="unit">{unit}</span>
    {/if}
  </div>
  <div class="meta">
    {#if probe.dph !== undefined && probe.c != null}
      <span class="num" class:up={probe.dph > 0} class:down={probe.dph < 0}>
        {probe.dph > 0 ? '▲' : probe.dph < 0 ? '▼' : '■'}
        {Math.abs(probe.dph).toFixed(1)}{unit}{t('per_hour')}
      </span>
    {/if}
    {#if done}
      <span>{t('eta')} <strong class="num">{fmtTime(done)}</strong></span>
    {/if}
  </div>
  <button class="alarms" onclick={edit} title={t('alarms')}>
    <span class="chip" class:on={loOn} class:ring={probe.a.r === 'L'}>
      ↓ {loOn ? `${lo}${unit}` : '—'}
    </span>
    <span class="chip" class:on={hiOn} class:ring={probe.a.r === 'H'}>
      ↑ {hiOn ? `${hi}${unit}` : '—'}
    </span>
  </button>
</section>

<Modal bind:open title={`${t('alarms')}: ${probe.n}`}>
  <div class="fields">
    <label class="field">
      <span class="row"><input type="checkbox" bind:checked={eLoOn} /> {t('low')} ({unit})</span>
      <input type="number" bind:value={eLo} disabled={!eLoOn} />
    </label>
    <label class="field">
      <span class="row"><input type="checkbox" bind:checked={eHiOn} /> {t('high')} ({unit})</span>
      <input type="number" bind:value={eHi} disabled={!eHiOn} />
    </label>
  </div>
  {#snippet actions()}
    <button class="ghost" onclick={() => (open = false)}>{t('cancel')}</button>
    <button class="primary" onclick={save}>{t('save')}</button>
  {/snippet}
</Modal>

<style>
  .probe {
    display: flex;
    flex-direction: column;
    gap: 6px;
    border-top: 3px solid var(--pc);
  }
  .probe.ringing {
    border-color: var(--danger);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--danger) 30%, transparent), var(--shadow);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .head h3 {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--pc);
  }
  .temp {
    font-size: 2.4rem;
    font-weight: 700;
    letter-spacing: -0.03em;
    line-height: 1.1;
  }
  .unit {
    font-size: 0.45em;
    color: var(--muted);
    font-weight: 600;
    margin-left: 3px;
  }
  .na {
    font-size: 1rem;
    color: var(--faint);
    font-weight: 550;
    letter-spacing: 0;
  }
  .meta {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
    font-size: 0.82rem;
    color: var(--muted);
    min-height: 1.2em;
  }
  .up {
    color: var(--accent);
  }
  .down {
    color: var(--info);
  }
  .alarms {
    justify-content: flex-start;
    background: transparent;
    border: none;
    padding: 0;
    gap: 6px;
    margin-top: auto;
    padding-top: 6px;
  }
  .alarms:hover:not(:disabled) {
    background: transparent;
  }
</style>
