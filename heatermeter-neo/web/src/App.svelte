<script lang="ts">
  import { route, link } from './lib/router.svelte'
  import { live, refreshInfo } from './lib/live.svelte'
  import { t, lang, setLang } from './lib/i18n.svelte'
  import { post, setValues } from './lib/api'
  import LoginModal from './components/LoginModal.svelte'
  import Dashboard from './pages/Dashboard.svelte'
  import HistoryPage from './pages/History.svelte'
  import SettingsPage from './pages/Settings.svelte'

  const page = $derived(
    route.path.startsWith('/settings') ? 'settings' : route.path.startsWith('/history') ? 'history' : 'dashboard',
  )

  const ringing = $derived(
    (live.status?.temps ?? [])
      .map((p, i) => ({ ...p, i }))
      .filter((p) => p.a.r),
  )

  const connected = $derived(live.streamOk && (live.info?.connected ?? false))
  const needsLogin = $derived(live.info?.password_set && !live.info?.admin)

  let theme = $state(document.documentElement.dataset.theme ?? 'auto')
  function cycleTheme() {
    theme = theme === 'auto' ? 'dark' : theme === 'dark' ? 'light' : 'auto'
    if (theme === 'auto') delete document.documentElement.dataset.theme
    else document.documentElement.dataset.theme = theme
    try {
      localStorage.setItem('theme', theme)
    } catch {
      /* ignore */
    }
  }

  async function silence() {
    await setValues({ al: '0,0,0,0,0,0,0,0' })
  }

  async function logout() {
    await post('/auth/logout')
    await refreshInfo()
  }

  $effect(() => {
    document.documentElement.lang = lang.code
  })
</script>

<header>
  <div class="bar">
    <a href="/" use:link class="brand" aria-label="HeaterMeter">
      <svg viewBox="0 0 64 64" width="28" height="28" aria-hidden="true">
        <defs>
          <linearGradient id="flame" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stop-color="var(--accent-2)" />
            <stop offset="1" stop-color="var(--accent)" />
          </linearGradient>
        </defs>
        <path
          d="M33 6c2 9-6 12-6 21 0 4 3 7 6 7-2-3 0-7 4-10 1 6 7 8 7 15 0 8-6 15-13 15S17 50 17 41c0-15 16-18 16-35z"
          fill="url(#flame)"
        />
      </svg>
      <span>HeaterMeter</span>
    </a>
    <nav>
      <a href="/" use:link class:active={page === 'dashboard'}>{t('dashboard')}</a>
      <a href="/history" use:link class:active={page === 'history'}>{t('history')}</a>
      <a href="/settings" use:link class:active={page === 'settings'}>{t('settings')}</a>
    </nav>
    <div class="tools">
      <span class="conn" class:ok={connected} title={connected ? t('connected') : t('disconnected')}></span>
      <button class="ghost small" onclick={() => setLang(lang.code === 'fi' ? 'en' : 'fi')} title={t('language')}>
        {lang.code === 'fi' ? 'EN' : 'FI'}
      </button>
      <button class="ghost small" onclick={cycleTheme} title={t('theme')} aria-label={t('theme')}>
        {theme === 'dark' ? '☾' : theme === 'light' ? '☀' : '◐'}
      </button>
      {#if needsLogin}
        <button class="small" onclick={() => (live.loginOpen = true)}>{t('login')}</button>
      {:else if live.info?.password_set}
        <button class="ghost small" onclick={logout}>{t('logout')}</button>
      {/if}
    </div>
  </div>
</header>

<main>
  {#if ringing.length}
    <div class="banner danger alarm" role="alert">
      <span class="bell">🔔</span>
      <div class="grow">
        {#each ringing as p (p.i)}
          <div>
            <strong>{p.n}</strong>
            {p.a.r === 'H' ? t('ringing_high') : t('ringing_low')} ({p.a.r === 'H' ? p.a.h : p.a.l})
          </div>
        {/each}
      </div>
      <button onclick={silence}>{t('silence')}</button>
    </div>
  {/if}
  {#if live.info?.simulator}
    <div class="banner info">{t('simulator_banner')}</div>
  {/if}
  {#if live.info && !live.info.connected}
    <div class="banner warn">{t('disconnected')}{live.info.port ? ` (${live.info.port})` : ''}</div>
  {:else if live.info && !live.info.configured}
    <div class="banner warn">{t('waiting_config')}</div>
  {/if}
  {#if live.info && !live.info.password_set && page !== 'settings'}
    <div class="banner warn">
      <span class="grow">{t('set_password_hint')}</span>
      <a href="/settings/system" use:link>{t('set_password')} →</a>
    </div>
  {/if}

  {#if page === 'dashboard'}
    <Dashboard />
  {:else if page === 'history'}
    <HistoryPage />
  {:else}
    <SettingsPage />
  {/if}
</main>

<LoginModal />

<style>
  header {
    position: sticky;
    top: 0;
    z-index: 10;
    backdrop-filter: saturate(140%) blur(12px);
    background: color-mix(in srgb, var(--bg) 78%, transparent);
    border-bottom: 1px solid var(--border);
  }
  .bar {
    max-width: 1280px;
    margin: 0 auto;
    padding: 10px 16px;
    display: flex;
    align-items: center;
    gap: 18px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text);
    font-weight: 700;
    letter-spacing: -0.02em;
    font-size: 1.05rem;
  }
  nav {
    display: flex;
    gap: 4px;
    flex: 1;
  }
  nav a {
    color: var(--muted);
    padding: 6px 12px;
    border-radius: 999px;
    font-weight: 550;
  }
  nav a:hover {
    color: var(--text);
  }
  nav a.active {
    color: var(--text);
    background: var(--surface-2);
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .conn {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--danger);
    margin-right: 6px;
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--danger) 25%, transparent);
  }
  .conn.ok {
    background: var(--ok);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--ok) 25%, transparent);
  }
  main > :global(*) {
    min-width: 0;
  }
  main {
    max-width: 1280px;
    margin: 0 auto;
    padding: 18px 16px 48px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .grow {
    flex: 1;
  }
  .alarm {
    animation: pulse 1.2s infinite;
  }
  .alarm button {
    background: #fff;
    color: var(--danger);
    border: none;
    font-weight: 700;
  }
  .bell {
    font-size: 1.3rem;
  }
  @media (max-width: 640px) {
    .brand span {
      display: none;
    }
    .bar {
      gap: 8px;
    }
    nav {
      min-width: 0;
      overflow-x: auto;
      scrollbar-width: none;
    }
    nav a {
      padding: 6px 8px;
      font-size: 0.9rem;
      white-space: nowrap;
    }
    .tools button.ghost.small {
      padding: 4px 6px;
    }
  }
</style>
