<script lang="ts">
  import { announce } from '../announce';
  import { connectionPresentation, createConnectionAnnouncer, type Updates } from '../updates';
  let { model, retry, refresh }: { model: Updates; retry: () => void; refresh: () => void } = $props();
  const view = $derived(connectionPresentation($model));
  const announceChange = createConnectionAnnouncer(announce);
  $effect(() => announceChange(view));
</script>

<div id="connection-status" class="connection-status" class:is-update={view.refresh} hidden={!view.visible}>
  <span id="connection-status-message" hidden={view.refresh}>{view.message}</span>
  <button id="connection-retry" type="button" hidden={!view.retry} onclick={retry}>retry</button>
  <button id="pwa-refresh" type="button" title="Refresh to update aggr and keep your place" hidden={!view.refresh} disabled={view.disabled} onclick={refresh}><span aria-hidden="true">↻</span> Refresh to update</button>
</div>
