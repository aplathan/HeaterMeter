<script lang="ts">
  import type { Status } from '../lib/api'
  import { setValues } from '../lib/api'
  import { fmtTemp } from '../lib/format'
  import { t } from '../lib/i18n.svelte'
  import Modal from './Modal.svelte'

  let { status, unit, lidtrack }: { status: Status | null; unit: string; lidtrack: boolean } = $props()

  const pit = $derived(status?.temps[0])
  const sp = $derived(status?.set ?? null)
  const mode = $derived(sp === null ? 'off' : sp < 0 ? 'manual' : 'auto')
  const output = $derived(status?.fan.c ?? 0)
  const lid = $derived(status?.lid ?? 0)
  const diff = $derived(pit?.c != null && sp !== null && sp > 0 ? pit.c - sp : null)

  let editOpen = $state(false)
  let editMode = $state<'auto' | 'manual' | 'off'>('auto')
  let editValue = $state('')
  let busy = $state(false)

  function openEdit() {
    editMode = mode as 'auto' | 'manual' | 'off'
    editValue = sp === null ? '225' : String(Math.abs(Math.round(sp)))
    editOpen = true
  }

  async function apply() {
    busy = true
    try {
      const v = editMode === 'off' ? 'off' : editMode === 'manual' ? `${parseInt(editValue) || 0}%` : `${parseInt(editValue) || 0}`
      await setValues({ sp: v })
      editOpen = false
    } finally {
      busy = false
    }
  }

  async function toggleLid() {
    await setValues({ ld: lid > 0 ? ',,0' : ',,1' })
  }

  const presets = $derived(unit === '°C' ? [107, 121, 135, 150, 177] : [225, 250, 275, 300, 350])
</script>

<section class="card pit" class:lid={lid > 0}>
  <div class="card-head">
    <h2>{pit?.n ?? t('pit')}</h2>
    <div class="row">
      {#if status?.ramp}<span class="chip on">↘ {t('ramp_active')}</span>{/if}
      {#if lidtrack}<span class="chip">LidTrack</span>{/if}
    </div>
  </div>

  <div class="main">
    <div class="temp num" class:unplugged={pit?.c == null}>
      {fmtTemp(pit?.c)}<span class="unit">{unit}</span>
    </div>
    <button class="sp" onclick={openEdit} title={t('setpoint')}>
      <span class="lbl">{t('setpoint')}</span>
      <span class="val num">
        {#if mode === 'off'}{t('off')}{:else if mode === 'manual'}{Math.abs(sp ?? 0)} %{:else}{Math.round(sp ?? 0)}{unit}{/if}
      </span>
      {#if diff !== null}
        <span class="diff num" class:hot={diff > 5} class:cold={diff < -5}>{diff > 0 ? '+' : ''}{diff.toFixed(1)}</span>
      {/if}
    </button>
  </div>

  <div class="meters">
    <div class="meter">
      <div class="m-head"><span>{t('output')}</span><span class="num">{Math.round(output)} %</span></div>
      <div class="track"><div class="fill" style:width="{Math.max(0, Math.min(100, output))}%"></div></div>
    </div>
    {#if status?.fan.f != null}
      <div class="meter small">
        <div class="m-head"><span>{t('fan')}</span><span class="num">{status.fan.f} %</span></div>
        <div class="track"><div class="fill alt" style:width="{status.fan.f}%"></div></div>
      </div>
    {/if}
    {#if status?.fan.s != null}
      <div class="meter small">
        <div class="m-head"><span>{t('servo')}</span><span class="num">{status.fan.s} %</span></div>
        <div class="track"><div class="fill alt" style:width="{status.fan.s}%"></div></div>
      </div>
    {/if}
  </div>

  <div class="foot">
    <button class:danger={lid > 0} onclick={toggleLid}>
      {#if lid > 0}{t('lid_open')} · {lid} s — {t('close_lid')}{:else}{t('lid_mode')}{/if}
    </button>
  </div>
</section>

<Modal bind:open={editOpen} title={t('setpoint')}>
  <div class="seg">
    {#each [['auto', t('auto')], ['manual', t('manual')], ['off', t('off')]] as [m, label] (m)}
      <button class:sel={editMode === m} onclick={() => (editMode = m as typeof editMode)}>{label}</button>
    {/each}
  </div>
  {#if editMode !== 'off'}
    <label class="field">
      {editMode === 'manual' ? t('manual_percent') : `${t('setpoint')} (${unit})`}
      <input type="number" inputmode="numeric" bind:value={editValue} min="0" max={editMode === 'manual' ? 100 : 1000} />
    </label>
    {#if editMode === 'auto'}
      <div class="row">
        {#each presets as p (p)}
          <button class="small" onclick={() => (editValue = String(p))}>{p}</button>
        {/each}
      </div>
    {/if}
  {/if}
  {#snippet actions()}
    <button class="ghost" onclick={() => (editOpen = false)}>{t('cancel')}</button>
    <button class="primary" onclick={apply} disabled={busy}>{t('set')}</button>
  {/snippet}
</Modal>

<style>
  .pit {
    position: relative;
    overflow: hidden;
    background:
      radial-gradient(500px 220px at 0% 0%, color-mix(in srgb, var(--accent) 16%, transparent), transparent 70%),
      var(--surface);
  }
  .pit.lid {
    border-color: color-mix(in srgb, var(--danger) 60%, var(--border));
  }
  .main {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
  }
  .temp {
    font-size: clamp(3.2rem, 9vw, 5rem);
    font-weight: 700;
    line-height: 1;
    letter-spacing: -0.04em;
    color: var(--text);
  }
  .temp.unplugged {
    color: var(--faint);
  }
  .unit {
    font-size: 0.4em;
    font-weight: 600;
    color: var(--muted);
    margin-left: 4px;
    letter-spacing: 0;
  }
  .sp {
    flex-direction: column;
    align-items: flex-end;
    gap: 0;
    padding: 10px 14px;
    background: var(--surface-2);
  }
  .sp .lbl {
    font-size: 0.75rem;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .sp .val {
    font-size: 1.6rem;
    font-weight: 700;
    color: var(--c-sp);
  }
  .diff {
    font-size: 0.8rem;
    color: var(--muted);
  }
  .diff.hot {
    color: var(--danger);
  }
  .diff.cold {
    color: var(--info);
  }
  .meters {
    display: grid;
    grid-template-columns: 2fr 1fr 1fr;
    gap: 14px;
    margin-top: 18px;
  }
  .m-head {
    display: flex;
    justify-content: space-between;
    font-size: 0.8rem;
    color: var(--muted);
    margin-bottom: 5px;
  }
  .track {
    height: 8px;
    border-radius: 99px;
    background: var(--surface-3);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    border-radius: 99px;
    background: linear-gradient(90deg, var(--accent-2), var(--accent));
    transition: width 0.6s ease;
  }
  .fill.alt {
    background: var(--muted);
  }
  .foot {
    margin-top: 16px;
    display: flex;
    justify-content: flex-end;
  }
  .seg {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 6px;
  }
  .seg button.sel {
    background: var(--accent);
    color: var(--accent-text);
    border-color: transparent;
  }
  @media (max-width: 520px) {
    .meters {
      grid-template-columns: 1fr;
    }
  }
</style>
