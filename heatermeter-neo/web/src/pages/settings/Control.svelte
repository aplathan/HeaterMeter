<script lang="ts">
  import { setValues } from '../../lib/api'
  import { live, refreshConfig, refreshInfo } from '../../lib/live.svelte'
  import { t } from '../../lib/i18n.svelte'
  import { action, cfg, daemon, saveSettings } from '../../lib/settings.svelte'
  import { num } from '../../lib/format'
  import SaveBar from '../../components/SaveBar.svelte'

  const LEDS: [number, string][] = [
    [0, 'Off'],
    [1, 'Alarm 0 Low'],
    [2, 'Alarm 0 High'],
    [3, 'Alarm 1 Low'],
    [4, 'Alarm 1 High'],
    [5, 'Alarm 2 Low'],
    [6, 'Alarm 2 High'],
    [7, 'Alarm 3 Low'],
    [8, 'Alarm 3 High'],
    [14, 'Any Alarm Ringing'],
    [9, 'RF Data Received'],
    [10, 'Lid Open'],
    [11, 'Output > 0%'],
    [13, 'Output = Max'],
    [12, 'Pit Temp Reached'],
    [15, 'Pit Startup Mode'],
    [16, 'Pit Recovery Mode'],
    [127, 'Unset'],
  ]
  const LED_COLORS = ['#ff4d5e', '#ffcc4d', '#a3e635']

  function fromConfig() {
    const oflag = num(cfg('oflag'))
    const lo = num(cfg('lo'))
    return {
      units: cfg('u') || 'F',
      pidp: cfg('pidp'),
      pidi: cfg('pidi'),
      pidd: cfg('pidd'),
      fmin: cfg('fmin'),
      fmax: cfg('fmax'),
      fsmax: cfg('fsmax'),
      fflor: cfg('fflor'),
      smin: String(num(cfg('smin')) * 10),
      smax: String(num(cfg('smax')) * 10),
      sceil: cfg('sceil'),
      invFan: !!(oflag & 1),
      invServo: !!(oflag & 2),
      feedVolt: !!(oflag & 0x10),
      lineFilter: oflag & 0x20 ? '50' : oflag & 0x40 ? '60' : '0',
      lidMethod: live.info?.lidtrack_enabled ? 'lidtrack' : lo > 0 ? 'threshold' : 'none',
      lo: String(lo || 6),
      ld: cfg('ld') || '240',
      lb: cfg('lb') || '50',
      lbn: cfg('lbn') || '255',
      le: [0, 1, 2].map((i) => num(cfg(`le${i}`)) & 0x7f),
      lei: [0, 1, 2].map((i) => !!(num(cfg(`le${i}`)) & 0x80)),
    }
  }

  let f = $state(fromConfig())
  let base = $state(fromConfig())
  const act = action()
  const rampAct = action()

  function reset() {
    f = fromConfig()
    base = fromConfig()
  }

  const fnVal = (x: typeof f) => {
    const oflag =
      (x.invFan ? 1 : 0) | (x.invServo ? 2 : 0) | (x.feedVolt ? 0x10 : 0) | (x.lineFilter === '50' ? 0x20 : x.lineFilter === '60' ? 0x40 : 0)
    return [x.fmin, x.fmax, Math.round(num(x.smin) / 10), Math.round(num(x.smax) / 10), oflag, x.fsmax, x.fflor, x.sceil].join(',')
  }
  const lbVal = (x: typeof f) => [x.lb, x.lbn, ...x.le.map((v, i) => v | (x.lei[i] ? 0x80 : 0))].join(',')
  const ldVal = (x: typeof f) => [x.lidMethod === 'threshold' ? x.lo : 0, x.ld].join(',')

  const changes = $derived.by(() => {
    const out: Record<string, string> = {}
    if (f.units !== base.units) {
      const sp = live.status?.set
      if (typeof sp === 'number' && sp > 0) {
        const conv = f.units === 'C' ? ((sp - 32) * 5) / 9 : (sp * 9) / 5 + 32
        out.sp = `${Math.round(conv)}${f.units}`
      } else {
        out.sp = f.units
      }
    }
    for (const k of ['pidp', 'pidi', 'pidd'] as const) if (f[k] !== base[k]) out[k] = f[k]
    if (fnVal(f) !== fnVal(base)) out.fn = fnVal(f)
    if (lbVal(f) !== lbVal(base)) out.lb = lbVal(f)
    if (ldVal(f) !== ldVal(base)) out.ld = ldVal(f)
    if ((f.lidMethod === 'lidtrack') !== (base.lidMethod === 'lidtrack'))
      out.lm_lidtrack_enabled = f.lidMethod === 'lidtrack' ? '1' : '0'
    return out
  })

  async function save() {
    await act.run(async () => {
      const r = await setValues(changes)
      await new Promise((res) => setTimeout(res, 600))
      await Promise.all([refreshConfig(), refreshInfo()])
      reset()
      if (!r.ok) throw new Error(r.log)
      return t('saved')
    })
  }

  function toggleWatch(i: number) {
    const s = daemon.settings
    if (!s) return
    s.ramp.watch = s.ramp.watch.includes(i) ? s.ramp.watch.filter((x) => x !== i) : [...s.ramp.watch, i].sort()
  }
</script>

<section class="card">
  <div class="card-head"><h2>{t('pid')}</h2></div>
  <div class="fields">
    <label class="field">
      {t('units')}
      <select bind:value={f.units}>
        <option value="F">°F</option>
        <option value="C">°C</option>
      </select>
    </label>
    <label class="field">P<input type="number" step="any" bind:value={f.pidp} /></label>
    <label class="field">I<input type="number" step="any" bind:value={f.pidi} /></label>
    <label class="field">D<input type="number" step="any" bind:value={f.pidd} /></label>
  </div>
</section>

<section class="card">
  <div class="card-head"><h2>{t('fan_settings')} / {t('servo_settings')}</h2></div>
  <div class="fields">
    <label class="field">{t('fan_settings')} {t('min')} %<input type="number" bind:value={f.fmin} /></label>
    <label class="field">{t('fan_settings')} {t('max')} %<input type="number" bind:value={f.fmax} /></label>
    <label class="field">{t('startup_max')} %<input type="number" bind:value={f.fsmax} /></label>
    <label class="field">{t('active_floor')} %<input type="number" bind:value={f.fflor} /></label>
    <label class="field">{t('servo_settings')} {t('min')} µs<input type="number" step="10" bind:value={f.smin} /></label>
    <label class="field">{t('servo_settings')} {t('max')} µs<input type="number" step="10" bind:value={f.smax} /></label>
    <label class="field">{t('active_ceil')} %<input type="number" bind:value={f.sceil} /></label>
    <label class="field">
      {t('line_filter')}
      <select bind:value={f.lineFilter}>
        <option value="0">{t('none')}</option>
        <option value="50">50 Hz</option>
        <option value="60">60 Hz</option>
      </select>
    </label>
  </div>
  <div class="row checks">
    <label class="check"><input type="checkbox" bind:checked={f.invFan} /> {t('fan_settings')}: {t('invert')}</label>
    <label class="check"><input type="checkbox" bind:checked={f.invServo} /> {t('servo_settings')}: {t('invert')}</label>
    <label class="check"><input type="checkbox" bind:checked={f.feedVolt} /> {t('fan_voltage')}</label>
  </div>
</section>

<section class="card">
  <div class="card-head"><h2>{t('lid_detection')}</h2></div>
  <div class="fields">
    <label class="field">
      {t('method')}
      <select bind:value={f.lidMethod}>
        <option value="none">{t('none')}</option>
        <option value="threshold">{t('threshold')}</option>
        <option value="lidtrack">{t('lidtrack')}</option>
      </select>
    </label>
    {#if f.lidMethod === 'threshold'}
      <label class="field">{t('lid_offset')}<input type="number" bind:value={f.lo} /></label>
    {/if}
    <label class="field">{t('lid_duration')}<input type="number" min="30" bind:value={f.ld} /></label>
  </div>
</section>

<section class="card">
  <div class="card-head"><h2>{t('display')}</h2></div>
  <div class="fields">
    <label class="field">{t('backlight')}<input type="number" min="0" max="100" bind:value={f.lb} /></label>
    <label class="field">
      {t('home_screen')}
      <select bind:value={f.lbn}>
        <option value="255">2-line</option>
        <option value="254">4-line</option>
        {#each [0, 1, 2, 3] as i (i)}<option value={String(i)}>Large: {cfg(`pn${i}`) || i}</option>{/each}
      </select>
    </label>
  </div>
  <div class="leds">
    {#each [0, 1, 2] as i (i)}
      <div class="led">
        <span class="dot" style:background={LED_COLORS[i]}></span>
        <select bind:value={f.le[i]}>
          {#each LEDS as [v, label] (v)}<option value={v}>{label}</option>{/each}
        </select>
        <label class="check"><input type="checkbox" bind:checked={f.lei[i]} /> {t('invert')}</label>
      </div>
    {/each}
  </div>
</section>

<SaveBar count={Object.keys(changes).length} busy={act.busy} msg={act.msg} ok={act.ok} onsave={save} onreset={reset} />

{#if daemon.settings}
  {@const s = daemon.settings}
  <section class="card">
    <div class="card-head">
      <h2>{t('ramp')}</h2>
      <label class="check"><input type="checkbox" bind:checked={s.ramp.enabled} /> {t('enabled')}</label>
    </div>
    <p class="muted">{t('ramp_help')}</p>
    <div class="fields">
      <div class="field">
        {t('ramp_watch')}
        <div class="row">
          {#each [1, 2, 3] as i (i)}
            <label class="check"><input type="checkbox" checked={s.ramp.watch.includes(i)} onchange={() => toggleWatch(i)} /> {cfg(`pn${i}`) || i}</label>
          {/each}
        </div>
      </div>
      <label class="field">{t('ramp_trigger')}<input type="number" bind:value={s.ramp.trigger} /></label>
      <label class="field">{t('ramp_target')}<input type="number" bind:value={s.ramp.target} /></label>
    </div>
    <div class="row end">
      {#if rampAct.msg}<span class:err={!rampAct.ok}>{rampAct.msg}</span>{/if}
      <button class="primary" disabled={rampAct.busy} onclick={() => rampAct.run(async () => { await saveSettings(); await refreshInfo(); return t('saved') })}>{t('save')}</button>
    </div>
  </section>
{/if}

<style>
  .checks {
    margin-top: 14px;
    gap: 18px;
  }
  .leds {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 12px;
    margin-top: 14px;
  }
  .led {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .led select {
    flex: 1;
  }
  .dot {
    flex: none;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    box-shadow: 0 0 10px currentColor;
  }
  .end {
    justify-content: flex-end;
    margin-top: 14px;
  }
  .err {
    color: var(--danger);
  }
  p {
    margin: 0 0 12px;
  }
</style>
