<script lang="ts">
  import { route, link } from '../lib/router.svelte'
  import { live } from '../lib/live.svelte'
  import { t, type Key } from '../lib/i18n.svelte'
  import { loadSettings } from '../lib/settings.svelte'
  import Probes from './settings/Probes.svelte'
  import Control from './settings/Control.svelte'
  import Alarms from './settings/Alarms.svelte'
  import Notifications from './settings/Notifications.svelte'
  import Network from './settings/Network.svelte'
  import Firmware from './settings/Firmware.svelte'
  import System from './settings/System.svelte'

  const TABS: [string, Key][] = [
    ['probes', 'probes'],
    ['control', 'control'],
    ['alarms', 'alarms'],
    ['notifications', 'notifications'],
    ['network', 'network'],
    ['firmware', 'firmware'],
    ['system', 'system'],
  ]

  const tab = $derived(route.path.split('/')[2] || 'probes')
  const allowed = $derived(live.info ? live.info.admin : false)

  $effect(() => {
    if (allowed) void loadSettings()
  })
</script>

<div class="layout">
  <nav class="tabs">
    {#each TABS as [id, label] (id)}
      <a href={`/settings/${id}`} use:link class:active={tab === id}>{t(label)}</a>
    {/each}
  </nav>
  <div class="content">
    {#if !live.info}
      <div class="card muted">…</div>
    {:else if !allowed}
      <div class="card stack">
        <h2>{t('login_required')}</h2>
        <div><button class="primary" onclick={() => (live.loginOpen = true)}>{t('login')}</button></div>
      </div>
    {:else if tab === 'probes'}
      <Probes />
    {:else if tab === 'control'}
      <Control />
    {:else if tab === 'alarms'}
      <Alarms />
    {:else if tab === 'notifications'}
      <Notifications />
    {:else if tab === 'network'}
      <Network />
    {:else if tab === 'firmware'}
      <Firmware />
    {:else}
      <System />
    {/if}
  </div>
</div>

<style>
  .layout {
    display: grid;
    grid-template-columns: 210px 1fr;
    gap: 18px;
    align-items: start;
  }
  .tabs {
    display: flex;
    flex-direction: column;
    gap: 2px;
    position: sticky;
    top: 70px;
  }
  .tabs a {
    color: var(--muted);
    padding: 9px 12px;
    border-radius: var(--radius-sm);
    font-weight: 550;
  }
  .tabs a:hover {
    color: var(--text);
    background: var(--surface);
  }
  .tabs a.active {
    color: var(--text);
    background: var(--surface-2);
    box-shadow: inset 3px 0 0 var(--accent);
  }
  .content {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  @media (max-width: 800px) {
    .layout {
      grid-template-columns: 1fr;
    }
    .tabs {
      flex-direction: row;
      overflow-x: auto;
      position: static;
    }
    .tabs a {
      white-space: nowrap;
    }
  }
</style>
