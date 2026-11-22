<script lang="ts">
  import { get, post, type WifiNetwork } from '../../lib/api'
  import { live } from '../../lib/live.svelte'
  import { t } from '../../lib/i18n.svelte'
  import { action, daemon, saveSettings } from '../../lib/settings.svelte'

  interface WifiStatus {
    available: boolean
    disabled?: boolean
    state?: string
    connection?: string
    hotspot?: boolean
    ip?: string | null
  }

  let status = $state<WifiStatus | null>(null)
  let networks = $state<WifiNetwork[]>([])
  let ssid = $state('')
  let password = $state('')
  const scanAct = action()
  const connAct = action()
  const saveAct = action()

  async function refresh() {
    try {
      status = await get<WifiStatus>('/wifi/status')
    } catch {
      status = { available: false }
    }
  }

  async function scan() {
    await scanAct.run(async () => {
      networks = await get<WifiNetwork[]>('/wifi/scan')
      return `${networks.length}`
    })
  }

  async function connect() {
    await connAct.run(async () => {
      const r = await post<{ message: string }>('/wifi/connect', { ssid, password })
      password = ''
      await refresh()
      return r.message
    })
  }

  async function hotspot() {
    await connAct.run(async () => {
      const r = await post<{ message: string }>('/wifi/hotspot')
      await refresh()
      return r.message
    })
  }

  $effect(() => {
    void refresh()
  })

  const bars = (s: number) => (s > 75 ? '▂▄▆█' : s > 50 ? '▂▄▆' : s > 25 ? '▂▄' : '▂')
</script>

<section class="card">
  <div class="card-head"><h2>{t('network')}</h2></div>
  <dl>
    <dt>{t('hostname')}</dt>
    <dd>{live.info?.hostname}</dd>
    <dt>{t('ip')}</dt>
    <dd class="num">{live.info?.ip ?? '–'}</dd>
    {#if status?.available}
      <dt>{t('wifi')}</dt>
      <dd>{status.connection || status.state}{status.hotspot ? ' (AP)' : ''}</dd>
    {/if}
  </dl>
</section>

{#if status && !status.available}
  <div class="card muted">{t('wifi_unavailable')}</div>
{:else if status}
  <section class="card">
    <div class="card-head">
      <h2>{t('wifi')}</h2>
      <button class="small" onclick={scan} disabled={scanAct.busy}>{scanAct.busy ? '…' : t('scan')}</button>
    </div>
    {#if scanAct.msg && !scanAct.ok}<p class="err">{scanAct.msg}</p>{/if}
    {#if networks.length}
      <ul class="nets">
        {#each networks as n (n.ssid)}
          <li>
            <button class="net" class:sel={ssid === n.ssid} onclick={() => (ssid = n.ssid)}>
              <span class="sig">{bars(n.signal)}</span>
              <span class="ssid">{n.ssid}</span>
              <span class="muted">{n.security || 'open'} · ch {n.channel}</span>
              {#if n.in_use}<span class="chip on">✓</span>{/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
    <form class="fields wide" onsubmit={(e) => { e.preventDefault(); void connect() }}>
      <label class="field">SSID<input bind:value={ssid} maxlength="32" /></label>
      <label class="field">{t('password')}<input type="password" bind:value={password} autocomplete="new-password" /></label>
      <div class="field end"><button class="primary" disabled={!ssid || connAct.busy}>{t('connect')}</button></div>
    </form>
    {#if connAct.msg}<p class:err={!connAct.ok}>{connAct.msg}</p>{/if}
  </section>

  {#if daemon.settings}
    {@const w = daemon.settings.wifi}
    <section class="card">
      <div class="card-head">
        <h2>{t('hotspot')}</h2>
        <button class="small" onclick={hotspot} disabled={connAct.busy}>{t('hotspot')}</button>
      </div>
      <div class="fields wide">
        <label class="check"><input type="checkbox" bind:checked={w.fallback_ap} /> {t('fallback_ap')}</label>
        <label class="field">{t('ap_ssid')}<input bind:value={w.ap_ssid} /></label>
        <label class="field">{t('ap_password')}<input type="password" bind:value={w.ap_password} minlength="8" /></label>
      </div>
      <div class="row end">
        {#if saveAct.msg}<span class:err={!saveAct.ok}>{saveAct.msg}</span>{/if}
        <button class="primary" onclick={() => saveAct.run(async () => { await saveSettings(); return t('saved') })}>{t('save')}</button>
      </div>
    </section>
  {/if}
{/if}

<style>
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 6px 18px;
    margin: 0;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
  }
  .nets {
    list-style: none;
    padding: 0;
    margin: 0 0 16px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .net {
    width: 100%;
    justify-content: flex-start;
    gap: 12px;
    background: transparent;
    border-color: transparent;
  }
  .net.sel {
    background: var(--surface-2);
    border-color: var(--accent);
  }
  .sig {
    font-family: var(--mono);
    color: var(--accent);
    width: 3.5em;
    text-align: left;
  }
  .ssid {
    font-weight: 600;
    flex: 1;
    text-align: left;
  }
  .end {
    justify-content: flex-end;
  }
  .field.end {
    justify-content: flex-end;
  }
  .row.end {
    margin-top: 14px;
  }
  .err {
    color: var(--danger);
  }
</style>
