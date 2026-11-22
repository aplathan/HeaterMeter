<script lang="ts">
  import { get, streamLines } from '../../lib/api'
  import { live, refreshInfo } from '../../lib/live.svelte'
  import { t } from '../../lib/i18n.svelte'

  interface FwInfo {
    enabled: boolean
    bundled: string[]
    default: string
    ucid: string | null
  }

  let info = $state<FwInfo | null>(null)
  let mode = $state<'upload' | 'url' | 'bundled'>('upload')
  let file = $state<FileList | null>(null)
  let url = $state('')
  let bundled = $state('')
  let log = $state<string[]>([])
  let busy = $state(false)
  let logEl: HTMLDivElement | undefined = $state()

  $effect(() => {
    get<FwInfo>('/firmware').then((i) => {
      info = i
      bundled = i.bundled[0] ?? ''
    })
  })

  $effect(() => {
    void log.length
    if (logEl) logEl.scrollTop = logEl.scrollHeight
  })

  async function flash() {
    const fd = new FormData()
    if (mode === 'upload' && file?.[0]) fd.append('hexfile', file[0])
    else if (mode === 'url') fd.append('url', url)
    else fd.append('bundled', bundled)
    busy = true
    log = []
    try {
      await streamLines('/firmware', fd, (l) => (log = [...log, l]))
    } catch (e) {
      log = [...log, String(e)]
    } finally {
      busy = false
      setTimeout(refreshInfo, 3000)
    }
  }

  const ready = $derived(
    !busy && (mode === 'upload' ? !!file?.length : mode === 'url' ? /^https?:\/\//.test(url) : !!bundled),
  )
</script>

<section class="card">
  <div class="card-head">
    <h2>{t('firmware')}</h2>
    <span class="chip on num">{live.info?.ucid ?? '–'}</span>
  </div>
  <p class="muted">{t('firmware_help')}</p>
  {#if info && !info.enabled}
    <div class="banner warn">{t('firmware_disabled')}</div>
  {:else}
    <div class="seg">
      <button class:sel={mode === 'upload'} onclick={() => (mode = 'upload')}>{t('upload_hex')}</button>
      <button class:sel={mode === 'url'} onclick={() => (mode = 'url')}>{t('from_url')}</button>
      <button class:sel={mode === 'bundled'} onclick={() => (mode = 'bundled')} disabled={!info?.bundled.length}>{t('bundled')}</button>
    </div>
    <div class="src">
      {#if mode === 'upload'}
        <input type="file" accept=".hex" bind:files={file} />
      {:else if mode === 'url'}
        <input type="url" bind:value={url} placeholder="https://…/hm.hex" />
      {:else}
        <select bind:value={bundled}>
          {#each info?.bundled ?? [] as b (b)}<option value={b}>{b.split('/').pop()}</option>{/each}
        </select>
      {/if}
      <button class="primary" onclick={flash} disabled={!ready}>{busy ? '…' : t('flash')}</button>
    </div>
  {/if}
  {#if log.length}
    <div class="log" bind:this={logEl}>{log.join('\n')}</div>
  {/if}
</section>

<style>
  p {
    margin: 0 0 14px;
  }
  .seg {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .seg .sel {
    background: var(--accent);
    color: var(--accent-text);
    border-color: transparent;
  }
  .src {
    display: flex;
    gap: 10px;
    margin: 14px 0;
  }
  .log {
    margin-top: 12px;
  }
</style>
