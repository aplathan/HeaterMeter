<script lang="ts">
  import { get, post } from '../../lib/api'
  import { live, refreshInfo } from '../../lib/live.svelte'
  import { t } from '../../lib/i18n.svelte'
  import { action, daemon, loadSettings, saveSettings } from '../../lib/settings.svelte'
  import { fmtDuration } from '../../lib/format'

  let current = $state('')
  let newPw = $state('')
  const pwAct = action()
  const apiAct = action()
  const cssAct = action()
  const sysAct = action()
  const calAct = action()
  let peaks = $state('')
  let fit = $state('')
  let calRunning = $state(false)

  async function setPassword(e: Event) {
    e.preventDefault()
    await pwAct.run(async () => {
      await post('/auth/password', { current, new: newPw })
      current = ''
      newPw = ''
      await refreshInfo()
      return t('saved')
    })
  }

  async function regen() {
    await apiAct.run(async () => {
      await post('/apikey/regenerate')
      await loadSettings()
      return t('saved')
    })
  }

  async function loadPeaks() {
    const r = await get<{ text: string }>('/diag/peaks')
    peaks = r.text
  }

  async function cal(op: 'start' | 'stop' | 'fit') {
    await calAct.run(async () => {
      if (op === 'fit') {
        const r = await post<Record<string, number | string>>('/unkprobe/fit')
        fit = JSON.stringify(r, null, 1)
        return `${r.n} ${t('points')}`
      }
      await post(`/unkprobe/${op}`)
      calRunning = op === 'start'
    })
  }
</script>

<section class="card">
  <div class="card-head"><h2>{t('admin_password')}</h2></div>
  <form class="fields wide" onsubmit={setPassword}>
    {#if live.info?.password_set}
      <label class="field">{t('current_password')}<input type="password" bind:value={current} autocomplete="current-password" /></label>
    {/if}
    <label class="field">{t('new_password')}<input type="password" bind:value={newPw} minlength="6" autocomplete="new-password" /></label>
    <div class="field end"><button class="primary" disabled={newPw.length < 6 || pwAct.busy}>{t('save')}</button></div>
  </form>
  {#if pwAct.msg}<p class:err={!pwAct.ok}>{pwAct.msg}</p>{/if}
  {#if daemon.settings}
    <label class="check gap"><input type="checkbox" bind:checked={daemon.settings.guest_read} onchange={() => saveSettings()} /> {t('guest_read')}</label>
  {/if}
</section>

{#if daemon.settings}
  {@const s = daemon.settings}
  <section class="card">
    <div class="card-head">
      <h2>{t('api_key')}</h2>
      <button class="small" onclick={regen}>{t('regenerate')}</button>
    </div>
    <code class="key">{s.api.key}</code>
    <div class="row gap">
      <label class="check"><input type="checkbox" bind:checked={s.api.disabled} onchange={() => apiAct.run(saveSettings)} /> {t('api_disabled')}</label>
      <label class="check"><input type="checkbox" bind:checked={s.api.allowcors} onchange={() => apiAct.run(saveSettings)} /> {t('allow_cors')}</label>
      {#if apiAct.msg}<span class:err={!apiAct.ok}>{apiAct.msg}</span>{/if}
    </div>
  </section>

  <section class="card">
    <div class="card-head">
      <h2>{t('user_css')}</h2>
      <button class="small" onclick={() => cssAct.run(async () => { await saveSettings(); return t('saved') })}>{t('save')}</button>
    </div>
    <textarea bind:value={s.ui.user_css} placeholder=":root {'{'} --accent: #e11d48; {'}'}"></textarea>
    {#if cssAct.msg}<p>{cssAct.msg}</p>{/if}
  </section>
{/if}

<section class="card">
  <div class="card-head">
    <h2>{t('calibration')}</h2>
    <div class="row">
      {#if !calRunning}
        <button class="small" onclick={() => cal('start')}>{t('start')}</button>
      {:else}
        <button class="small" onclick={() => cal('fit')}>{t('fit')}</button>
        <button class="small ghost" onclick={() => cal('stop')}>{t('stop')}</button>
      {/if}
    </div>
  </div>
  <p class="muted">{t('calibration_help')}</p>
  {#if calAct.msg}<p class:err={!calAct.ok}>{calAct.msg}</p>{/if}
  {#if fit}<pre class="log">{fit}</pre>{/if}
</section>

<section class="card">
  <div class="card-head">
    <h2>{t('diagnostics')}</h2>
    <div class="row">
      <button class="small" onclick={loadPeaks}>Peaks</button>
      <button class="small" onclick={() => sysAct.run(async () => { await post('/reboot'); return 'OK' })}>{t('reboot_hm')}</button>
    </div>
  </div>
  <dl>
    <dt>{t('version')}</dt>
    <dd>linkmeterd {live.info?.version} · HeaterMeter {live.info?.ucid ?? '–'}</dd>
    <dt>{t('uptime')}</dt>
    <dd>{live.info?.uptime ? fmtDuration(live.info.uptime) : fmtDuration(live.info?.daemon_uptime ?? 0)}</dd>
    {#if live.info?.cpu_temp}
      <dt>{t('cpu_temp')}</dt>
      <dd>{live.info.cpu_temp.toFixed(1)} °C</dd>
    {/if}
    <dt>Serial</dt>
    <dd>{live.info?.port ?? '–'} · cerr {live.config.cerr ?? 0}</dd>
  </dl>
  {#if sysAct.msg}<p>{sysAct.msg}</p>{/if}
  {#if peaks}<pre class="log">{peaks}</pre>{/if}
  {#if live.log.length}
    <div class="log">{live.log.map((l) => `${new Date(l.time * 1000).toLocaleTimeString()} ${l.msg}`).join('\n')}</div>
  {/if}
</section>

<style>
  .end {
    justify-content: flex-end;
  }
  .gap {
    margin-top: 14px;
  }
  .key {
    display: block;
    font-family: var(--mono);
    background: var(--surface-2);
    border: 1px solid var(--border);
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    word-break: break-all;
    user-select: all;
  }
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 6px 18px;
    margin: 0 0 12px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
  }
  p {
    margin: 10px 0 0;
  }
  .err {
    color: var(--danger);
  }
  pre {
    margin: 10px 0 0;
  }
</style>
