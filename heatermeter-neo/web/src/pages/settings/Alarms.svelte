<script lang="ts">
  import { post, setValues, type AlarmAction } from '../../lib/api'
  import { live, refreshConfig, units } from '../../lib/live.svelte'
  import { t } from '../../lib/i18n.svelte'
  import { action, cfg, daemon, saveSettings } from '../../lib/settings.svelte'
  import { num, unitSymbol } from '../../lib/format'
  import SaveBar from '../../components/SaveBar.svelte'

  const KINDS = ['L', 'H'] as const
  const unit = $derived(unitSymbol(units()))

  function fromConfig() {
    return [0, 1, 2, 3].map((p) =>
      KINDS.map((k) => {
        const v = num(cfg(`pal${k.toLowerCase()}${p}`))
        return { on: v > 0, value: v === 0 ? '' : String(Math.abs(v)) }
      }),
    )
  }

  let th = $state(fromConfig())
  let base = $state(fromConfig())
  const act = action()
  const testAct = action()

  const enc = (x: { on: boolean; value: string }) => {
    const n = Math.abs(parseInt(x.value))
    return Number.isFinite(n) && n > 0 ? String(x.on ? n : -n) : '0'
  }
  const alVal = (x: typeof th) => x.flatMap((p) => p.map(enc)).join(',')

  function getAction(p: number, k: string): AlarmAction {
    return daemon.settings!.alarms.actions[`${p}${k}`]
  }

  let actionsDirty = $state(false)
  const count = $derived((alVal(th) !== alVal(base) ? 1 : 0) + (actionsDirty ? 1 : 0))

  async function save() {
    await act.run(async () => {
      if (alVal(th) !== alVal(base)) {
        // only send thresholds that changed (empty field = unchanged)
        const vals = th.flatMap((p, i) => p.map((x, j) => (enc(x) !== enc(base[i][j]) ? enc(x) : '')))
        const r = await setValues({ al: vals.join(',') })
        if (!r.ok) throw new Error(r.log)
      }
      if (actionsDirty) await saveSettings()
      actionsDirty = false
      await new Promise((res) => setTimeout(res, 500))
      await refreshConfig()
      th = fromConfig()
      base = fromConfig()
      return t('saved')
    })
  }

  function reset() {
    th = fromConfig()
    base = fromConfig()
  }

  async function test(p: number, k: string) {
    await testAct.run(async () => {
      await post('/alarms/test', { probe: p, kind: k })
      setTimeout(() => void post('/alarms/test', { probe: p, stop: true }), 5000)
      return `${cfg(`pn${p}`)} ${k} → ${t('test')}`
    })
  }
</script>

<section class="card">
  <div class="card-head">
    <h2>{t('alarms')}</h2>
    {#if testAct.msg}<span class="muted">{testAct.msg}</span>{/if}
  </div>
  <div class="table-wrap">
    <table>
      <thead>
        <tr>
          <th>{t('name')}</th>
          <th></th>
          <th>{t('threshold')} ({unit})</th>
          {#if daemon.settings}
            <th>{t('email')}</th>
            <th>{t('sms')}</th>
            <th>{t('push')}</th>
            <th>{t('on_alarm_setpoint')}</th>
            <th>{t('on_alarm_action')}</th>
          {/if}
          <th></th>
        </tr>
      </thead>
      <tbody>
        {#each th as probe, p (p)}
          {#each KINDS as k, j (k)}
            {@const x = probe[j]}
            <tr class:first={j === 0} class:ring={live.status?.temps[p]?.a.r === k}>
              <td>{#if j === 0}<strong>{cfg(`pn${p}`) || p}</strong>{/if}</td>
              <td class="muted">{k === 'L' ? `↓ ${t('low')}` : `↑ ${t('high')}`}</td>
              <td>
                <div class="row nowrap">
                  <input type="checkbox" bind:checked={x.on} aria-label={t('enabled')} />
                  <input class="numin" type="number" bind:value={x.value} disabled={!x.on} />
                </div>
              </td>
              {#if daemon.settings}
                {@const a = getAction(p, k)}
                <td><input type="checkbox" bind:checked={a.email} onchange={() => (actionsDirty = true)} /></td>
                <td><input type="checkbox" bind:checked={a.sms} onchange={() => (actionsDirty = true)} /></td>
                <td><input type="checkbox" bind:checked={a.push} onchange={() => (actionsDirty = true)} /></td>
                <td>
                  <input
                    class="numin"
                    placeholder="—"
                    value={a.setpoint ?? ''}
                    oninput={(e) => {
                      const v = (e.target as HTMLInputElement).value.trim()
                      a.setpoint = v || null
                      actionsDirty = true
                    }}
                  />
                </td>
                <td>
                  <select bind:value={a.action} onchange={() => (actionsDirty = true)}>
                    <option value="ring">{t('action_ring')}</option>
                    <option value="silence">{t('action_silence')}</option>
                    <option value="disable">{t('action_disable')}</option>
                  </select>
                </td>
              {/if}
              <td><button class="small ghost" onclick={() => test(p, k)}>{t('test')}</button></td>
            </tr>
          {/each}
        {/each}
      </tbody>
    </table>
  </div>
</section>

<SaveBar {count} busy={act.busy} msg={act.msg} ok={act.ok} onsave={save} onreset={reset} />

<style>
  .table-wrap {
    overflow-x: auto;
  }
  tr.first td {
    border-top: 1px solid var(--border);
  }
  td {
    border-bottom: none;
    padding: 6px 8px;
    vertical-align: middle;
  }
  tr.ring td {
    background: color-mix(in srgb, var(--danger) 12%, transparent);
  }
  .numin {
    width: 90px;
  }
  .nowrap {
    flex-wrap: nowrap;
  }
  select {
    width: auto;
  }
</style>
