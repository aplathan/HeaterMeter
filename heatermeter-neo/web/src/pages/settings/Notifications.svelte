<script lang="ts">
  import { post } from '../../lib/api'
  import { t } from '../../lib/i18n.svelte'
  import { action, daemon, saveSettings } from '../../lib/settings.svelte'

  const act = action()
  const testAct = action()

  async function test(channel: 'email' | 'sms' | 'push') {
    await testAct.run(async () => {
      await saveSettings()
      const r = await post<{ message: string }>('/notify/test', { channel })
      return r.message
    })
  }
</script>

{#if daemon.settings}
  {@const s = daemon.settings}
  <section class="card">
    <div class="card-head"><h2>{t('smtp_server')}</h2></div>
    <div class="fields wide">
      <label class="field">{t('host')}<input bind:value={s.smtp.host} placeholder="smtp.example.com" /></label>
      <label class="field">{t('port')}<input type="number" bind:value={s.smtp.port} /></label>
      <label class="field">
        {t('security')}
        <select bind:value={s.smtp.security}>
          <option value="starttls">STARTTLS</option>
          <option value="tls">TLS</option>
          <option value="none">{t('none')}</option>
        </select>
      </label>
      <label class="field">{t('username')}<input bind:value={s.smtp.username} autocomplete="off" /></label>
      <label class="field">{t('password')}<input type="password" bind:value={s.smtp.password} autocomplete="new-password" /></label>
      <label class="field">{t('from_address')}<input bind:value={s.smtp.from_address} placeholder="grill@example.com" /></label>
    </div>
  </section>

  <section class="card">
    <div class="card-head">
      <h2>{t('email')}</h2>
      <button class="small" onclick={() => test('email')} disabled={testAct.busy}>{t('test')}</button>
    </div>
    <div class="fields wide">
      <label class="field">{t('to_address')}<input type="email" bind:value={s.alarms.email.to_address} /></label>
      <label class="field">{t('to_name')}<input bind:value={s.alarms.email.to_name} /></label>
      <label class="field">{t('from_name')}<input bind:value={s.alarms.email.from_name} /></label>
      <label class="field">{t('subject')}<input bind:value={s.alarms.email.subject} /></label>
    </div>
    <label class="field top">{t('body')}<textarea bind:value={s.alarms.email.body}></textarea></label>
    <p class="muted hint">{t('template_help')}</p>
  </section>

  <section class="card">
    <div class="card-head">
      <h2>{t('sms')}</h2>
      <button class="small" onclick={() => test('sms')} disabled={testAct.busy}>{t('test')}</button>
    </div>
    <div class="fields wide">
      <label class="field">{t('to_address')}<input bind:value={s.alarms.sms.to_address} /></label>
      <label class="field">{t('message')}<input bind:value={s.alarms.sms.message} /></label>
    </div>
    <p class="muted hint">{t('sms_help')}</p>
  </section>

  <section class="card">
    <div class="card-head">
      <h2>{t('push')}</h2>
      <button class="small" onclick={() => test('push')} disabled={testAct.busy || s.alarms.push.provider === 'none'}>{t('test')}</button>
    </div>
    <div class="fields wide">
      <label class="field">
        {t('provider')}
        <select bind:value={s.alarms.push.provider}>
          <option value="none">{t('none')}</option>
          <option value="ntfy">ntfy</option>
          <option value="pushover">Pushover</option>
          <option value="pushbullet">Pushbullet</option>
          <option value="webhook">Webhook</option>
        </select>
      </label>
      <label class="field">{t('title')}<input bind:value={s.alarms.push.title} /></label>
      <label class="field">{t('message')}<input bind:value={s.alarms.push.message} /></label>
      {#if s.alarms.push.provider === 'ntfy'}
        <label class="field">Topic URL<input bind:value={s.alarms.push.ntfy_url} placeholder="https://ntfy.sh/my-grill" /></label>
        <label class="field">Token<input type="password" bind:value={s.alarms.push.ntfy_token} /></label>
      {:else if s.alarms.push.provider === 'pushover'}
        <label class="field">User key<input bind:value={s.alarms.push.pushover_user} /></label>
        <label class="field">API token<input bind:value={s.alarms.push.pushover_token} /></label>
        <label class="field">Sound<input bind:value={s.alarms.push.pushover_sound} /></label>
      {:else if s.alarms.push.provider === 'pushbullet'}
        <label class="field">Access token<input bind:value={s.alarms.push.pushbullet_key} /></label>
      {:else if s.alarms.push.provider === 'webhook'}
        <label class="field">URL<input bind:value={s.alarms.push.webhook_url} placeholder="https://…" /></label>
      {/if}
    </div>
  </section>

  <div class="savebar card">
    <span class="status" class:err={!(testAct.msg ? testAct.ok : act.ok)}>{testAct.msg || act.msg}</span>
    <button class="primary" disabled={act.busy} onclick={() => act.run(async () => { await saveSettings(); return t('saved') })}>{t('save')}</button>
  </div>
{:else}
  <div class="card muted">{daemon.error || '…'}</div>
{/if}

<style>
  .top {
    margin-top: 12px;
  }
  .hint {
    font-size: 0.8rem;
    margin: 8px 0 0;
  }
  .savebar {
    position: sticky;
    bottom: 12px;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
  }
  .status {
    flex: 1;
    color: var(--muted);
    font-size: 0.88rem;
  }
  .status.err {
    color: var(--danger);
  }
</style>
