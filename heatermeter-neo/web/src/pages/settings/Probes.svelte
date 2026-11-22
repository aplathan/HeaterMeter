<script lang="ts">
  import { setValues } from '../../lib/api'
  import { live, refreshConfig } from '../../lib/live.svelte'
  import { t } from '../../lib/i18n.svelte'
  import { action, cfg } from '../../lib/settings.svelte'
  import SaveBar from '../../components/SaveBar.svelte'

  // Steinhart-Hart presets from the original LinkMeter conf.htm
  const PRESETS = [
    { name: 'ET-72/73', desc: 'Maverick Industries', stein: '2.4723753e-04,2.3402251e-04,1.3879768e-07' },
    { name: 'ET-732', desc: 'Maverick Industries', stein: '5.2668241e-04,2.0037400e-04,2.5703090e-08' },
    { name: 'ET-735', desc: 'Maverick Industries', stein: '7.5714216e-4,1.8829450e-4,1.4440251e-9' },
    { name: 'iGrill', desc: 'iGrill Probe', stein: '0.7739251279e-3,2.088025997e-4,1.154400438e-7' },
    { name: 'Inkbird', desc: 'IBT-2X, IBT-4XS, IBT-6X', stein: '7.2323395e-4,2.3971089e-4,4.6889292e-8' },
    { name: 'Thermoworks', desc: 'Pro-Series', stein: '7.3431401e-4,2.1574370e-4,9.5156860e-8' },
    { name: 'Vishay 10k', desc: 'NTCLE203E3103FB0', stein: '1.14061e-3,2.32134e-4,9.63666e-8' },
    { name: 'EPCOS 100k', desc: 'Beta 3950', stein: '7.2237825e-4,2.1630182e-4,9.2641029e-8' },
    { name: 'Semitec 104GT-2', desc: '100k', stein: '8.1129016e-4,2.1135575e-4,7.1761474e-8' },
  ]

  interface Probe {
    name: string
    type: string
    a: string
    b: string
    c: string
    r: string
    offset: string
    rf: string
  }

  function fromConfig(): Probe[] {
    return [0, 1, 2, 3].map((i) => ({
      name: cfg(`pn${i}`),
      type: cfg(`pt${i}`) || '0',
      a: cfg(`pca${i}`),
      b: cfg(`pcb${i}`),
      c: cfg(`pcc${i}`),
      r: cfg(`pcr${i}`),
      offset: cfg(`po${i}`) || '0',
      rf: cfg(`prfn${i}`) || '127',
    }))
  }

  let probes = $state(fromConfig())
  let base = $state(fromConfig())
  let rfNodes = $state<{ id: number; rssi: number; lobatt: number }[]>([])
  const act = action()

  $effect(() => {
    fetch('/luci/lm/rfstatus')
      .then((r) => r.json())
      .then((n) => (rfNodes = n.filter((x: { id: number }) => x.id !== 255 && x.id !== 127)))
      .catch(() => {})
  })

  function reset() {
    base = fromConfig()
    probes = fromConfig()
  }

  const coeffKey = (p: Probe) => [p.a, p.b, p.c, p.r, p.type, p.type === '2' ? p.rf : ''].join('|')

  const changes = $derived.by(() => {
    const out: Record<string, string> = {}
    probes.forEach((p, i) => {
      const b = base[i]
      if (p.name !== b.name) out[`pn${i}`] = p.name.slice(0, 12)
      if (coeffKey(p) !== coeffKey(b)) {
        const type = p.type === '2' ? String(128 + (parseInt(p.rf) || 127)) : p.type
        out[`pc${i}`] = [p.a, p.b, p.c, p.r, type].join(',')
      }
    })
    if (probes.some((p, i) => p.offset !== base[i].offset)) out.po = probes.map((p) => parseInt(p.offset) || 0).join(',')
    return out
  })

  function applyPreset(p: Probe, stein: string) {
    const [a, b, c] = stein.split(',')
    p.a = a
    p.b = b
    p.c = c
    if (!p.r || p.r === '0') p.r = '10000'
  }

  async function save() {
    await act.run(async () => {
      const r = await setValues(changes)
      await new Promise((res) => setTimeout(res, 600))
      await refreshConfig()
      reset()
      if (!r.ok) throw new Error(r.log)
      return t('saved')
    })
  }

  const presetOf = (p: Probe) =>
    PRESETS.find((x) => {
      const [a, b, c] = x.stein.split(',').map(Number)
      return Math.abs(a - Number(p.a)) < 1e-9 && Math.abs(b - Number(p.b)) < 1e-10 && Math.abs(c - Number(p.c)) < 1e-12
    })?.stein ?? ''
</script>

{#each probes as p, i (i)}
  <section class="card">
    <div class="card-head">
      <h2><span class="idx">{i}</span> {p.name || '—'}</h2>
      {#if live.status?.temps[i]?.c != null}<span class="chip">{live.status.temps[i].c}</span>{/if}
    </div>
    <div class="fields">
      <label class="field">{t('name')}<input bind:value={p.name} maxlength="12" /></label>
      <label class="field">
        {t('type')}
        <select bind:value={p.type}>
          <option value="0">{t('disabled')}</option>
          <option value="1">{t('thermistor')}</option>
          <option value="2">{t('rf')}</option>
          <option value="3">{t('thermocouple')}</option>
        </select>
      </label>
      <label class="field">{t('offset')}<input type="number" bind:value={p.offset} /></label>
      {#if p.type === '2'}
        <label class="field">
          {t('rf_node')}
          <select bind:value={p.rf}>
            <option value="127">{t('any')}</option>
            {#each rfNodes as n (n.id)}
              <option value={String(n.id)}>#{n.id} ({n.rssi}){n.lobatt ? ' 🔋' : ''}</option>
            {/each}
            {#if p.rf !== '127' && !rfNodes.some((n) => String(n.id) === p.rf)}
              <option value={p.rf}>#{p.rf}</option>
            {/if}
          </select>
        </label>
      {/if}
    </div>
    {#if p.type === '1' || p.type === '2'}
      <details open={i === 0}>
        <summary>{t('coefficients')}</summary>
        <div class="fields wide coeff">
          <label class="field">
            {t('preset')}
            <select value={presetOf(p)} onchange={(e) => applyPreset(p, (e.target as HTMLSelectElement).value)}>
              <option value="">—</option>
              {#each PRESETS as pr (pr.name)}
                <option value={pr.stein}>{pr.name} · {pr.desc}</option>
              {/each}
            </select>
          </label>
          <label class="field">A<input class="mono" bind:value={p.a} /></label>
          <label class="field">B<input class="mono" bind:value={p.b} /></label>
          <label class="field">C<input class="mono" bind:value={p.c} /></label>
          <label class="field">{t('resistor')}<input type="number" bind:value={p.r} /></label>
        </div>
      </details>
    {:else if p.type === '3'}
      <div class="fields coeff">
        <label class="field">{t('mv_per_c')}<input bind:value={p.r} /></label>
      </div>
    {/if}
  </section>
{/each}

<SaveBar count={Object.keys(changes).length} busy={act.busy} msg={act.msg} ok={act.ok} onsave={save} onreset={reset} />

<style>
  .idx {
    display: inline-grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 6px;
    background: var(--surface-3);
    font-size: 0.8rem;
    margin-right: 4px;
  }
  details {
    margin-top: 14px;
  }
  summary {
    cursor: pointer;
    color: var(--muted);
    font-weight: 550;
    font-size: 0.88rem;
  }
  .coeff {
    margin-top: 10px;
  }
  .mono {
    font-family: var(--mono);
    font-size: 0.85rem;
  }
</style>
