<script lang="ts">
  import Modal from './Modal.svelte'
  import { post } from '../lib/api'
  import { live, refreshInfo } from '../lib/live.svelte'
  import { t } from '../lib/i18n.svelte'

  let password = $state('')
  let error = $state('')
  let busy = $state(false)

  async function submit(e: Event) {
    e.preventDefault()
    busy = true
    error = ''
    try {
      await post('/auth/login', { password })
      password = ''
      live.loginOpen = false
      await refreshInfo()
    } catch {
      error = t('wrong_password')
    } finally {
      busy = false
    }
  }
</script>

<Modal bind:open={live.loginOpen} title={t('login')}>
  <form id="login-form" class="stack" onsubmit={submit}>
    <label class="field">
      {t('admin_password')}
      <!-- svelte-ignore a11y_autofocus -->
      <input type="password" bind:value={password} autocomplete="current-password" autofocus />
    </label>
    {#if error}<div class="err">{error}</div>{/if}
  </form>
  {#snippet actions()}
    <button class="ghost" onclick={() => (live.loginOpen = false)}>{t('cancel')}</button>
    <button class="primary" type="submit" form="login-form" disabled={busy || !password}>{t('login')}</button>
  {/snippet}
</Modal>

<style>
  .err {
    color: var(--danger);
    font-weight: 550;
  }
</style>
