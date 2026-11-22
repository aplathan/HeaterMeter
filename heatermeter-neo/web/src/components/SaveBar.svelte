<script lang="ts">
  import { t } from '../lib/i18n.svelte'

  let {
    count = -1,
    busy = false,
    msg = '',
    ok = true,
    onsave,
    onreset,
  }: { count?: number; busy?: boolean; msg?: string; ok?: boolean; onsave: () => void; onreset?: () => void } = $props()
</script>

<div class="savebar">
  <span class="status" class:err={!ok}>
    {#if msg}{msg}{:else if count === 0}{t('no_changes')}{:else if count > 0}{count} {t('changed_fields')}{/if}
  </span>
  {#if onreset}
    <button class="ghost" onclick={onreset} disabled={busy || count === 0}>{t('cancel')}</button>
  {/if}
  <button class="primary" onclick={onsave} disabled={busy || count === 0}>{t('save')}</button>
</div>

<style>
  .savebar {
    position: sticky;
    bottom: 12px;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 10px;
    padding: 10px 14px;
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--surface) 88%, transparent);
    backdrop-filter: blur(10px);
    border: 1px solid var(--border);
    box-shadow: var(--shadow);
  }
  .status {
    flex: 1;
    color: var(--muted);
    font-size: 0.88rem;
    white-space: pre-line;
  }
  .status.err {
    color: var(--danger);
  }
</style>
