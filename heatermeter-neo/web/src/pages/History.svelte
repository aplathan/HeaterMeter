<script lang="ts">
  import { get, post, send, type History, type Session } from '../lib/api'
  import { live, units, refreshInfo } from '../lib/live.svelte'
  import { fmtDateTime, fmtDuration, unitSymbol } from '../lib/format'
  import { t } from '../lib/i18n.svelte'
  import Chart from '../components/Chart.svelte'

  let sessions = $state<Session[]>([])
  let selected = $state<number | null>(null)
  let history = $state<History | null>(null)
  let editing = $state<number | null>(null)
  let editName = $state('')

  async function loadSessions() {
    sessions = await get<Session[]>('/sessions')
    if (selected === null && sessions.length) select(sessions[0].id)
  }

  async function select(id: number) {
    selected = id
    history = null
    history = await get<History>(`/sessions/${id}/history?points=2500`)
  }

  async function rename(s: Session) {
    await send('PATCH', `/sessions/${s.id}`, { name: editName })
    editing = null
    await loadSessions()
  }

  async function remove(s: Session) {
    if (!confirm(t('confirm_delete'))) return
    await send('DELETE', `/sessions/${s.id}`)
    if (selected === s.id) {
      selected = null
      history = null
    }
    await loadSessions()
  }

  async function newSession() {
    await post('/sessions/new')
    await refreshInfo()
    await loadSessions()
  }

  async function cont(s: Session) {
    await post(`/sessions/${s.id}/continue`)
    await refreshInfo()
    await loadSessions()
  }

  $effect(() => {
    void loadSessions()
  })

  const sel = $derived(sessions.find((s) => s.id === selected))
  const unit = $derived(unitSymbol(units()))
  const names = $derived((live.status?.temps ?? []).map((p) => p.n))
</script>

<div class="layout">
  <section class="card list">
    <div class="card-head">
      <h2>{t('sessions')}</h2>
      <button class="small primary" onclick={newSession}>＋ {t('new_session')}</button>
    </div>
    <ul>
      {#each sessions as s (s.id)}
        <li class:sel={s.id === selected}>
          {#if editing === s.id}
            <form class="row" onsubmit={(e) => { e.preventDefault(); void rename(s) }}>
              <!-- svelte-ignore a11y_autofocus -->
              <input bind:value={editName} autofocus maxlength="80" />
              <button class="small primary">{t('save')}</button>
            </form>
          {:else}
            <button class="item" onclick={() => select(s.id)}>
              <span class="name">
                {s.name || t('untitled')}
                {#if s.current}<span class="chip on">{t('current')}</span>{/if}
              </span>
              <span class="muted small">{fmtDateTime(s.started)} · {fmtDuration(s.last - s.started)}</span>
            </button>
          {/if}
        </li>
      {:else}
        <li class="muted">{t('no_data')}</li>
      {/each}
    </ul>
  </section>

  <section class="card view">
    {#if sel}
      <div class="card-head">
        <div>
          <h2>{sel.name || t('untitled')}</h2>
          <div class="muted small">
            {fmtDateTime(sel.started)} – {fmtDateTime(sel.last)} · {sel.samples} {t('samples')}
          </div>
        </div>
        <div class="row">
          <a class="btn small" href={`/api/v2/sessions/${sel.id}/csv`} download>{t('download_csv')}</a>
          <button class="small" onclick={() => { editing = sel.id; editName = sel.name }}>{t('rename')}</button>
          {#if !sel.current}
            <button class="small" onclick={() => cont(sel)}>{t('continue_session')}</button>
          {/if}
          <button class="small ghost" onclick={() => remove(sel)}>{t('delete')}</button>
        </div>
      </div>
      <Chart {history} {names} {unit} height={420} />
    {:else}
      <div class="muted">{t('no_data')}</div>
    {/if}
  </section>
</div>

<style>
  .layout {
    display: grid;
    grid-template-columns: 300px 1fr;
    gap: 16px;
    align-items: start;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 70vh;
    overflow: auto;
  }
  li.sel .item {
    background: var(--surface-2);
    border-color: var(--border);
  }
  .item {
    width: 100%;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    background: transparent;
    border-color: transparent;
    text-align: left;
    white-space: normal;
  }
  .name {
    display: flex;
    gap: 6px;
    align-items: center;
    font-weight: 600;
  }
  .small {
    font-size: 0.82rem;
  }
  .view {
    min-width: 0;
  }
  @media (max-width: 860px) {
    .layout {
      grid-template-columns: 1fr;
    }
    ul {
      max-height: 260px;
    }
  }
</style>
