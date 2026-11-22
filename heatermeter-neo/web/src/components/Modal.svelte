<script lang="ts">
  import type { Snippet } from 'svelte'

  let {
    open = $bindable(false),
    title,
    children,
    actions,
  }: { open?: boolean; title: string; children: Snippet; actions?: Snippet } = $props()

  let dialog: HTMLDialogElement | undefined = $state()

  $effect(() => {
    if (!dialog) return
    if (open && !dialog.open) dialog.showModal()
    if (!open && dialog.open) dialog.close()
  })
</script>

<dialog bind:this={dialog} onclose={() => (open = false)} onclick={(e) => e.target === dialog && (open = false)}>
  <div class="inner">
    <h2>{title}</h2>
    <div class="body">{@render children()}</div>
    {#if actions}
      <div class="actions">{@render actions()}</div>
    {/if}
  </div>
</dialog>

<style>
  dialog {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text);
    padding: 0;
    width: min(440px, calc(100vw - 32px));
    box-shadow: 0 30px 80px rgba(0, 0, 0, 0.45);
  }
  dialog::backdrop {
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(3px);
  }
  .inner {
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
