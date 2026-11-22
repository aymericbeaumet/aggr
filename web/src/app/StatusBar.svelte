<script lang="ts">
  import { pull } from '../offline/pull-refresh.svelte';
  import { announcer } from '../state/announcer.svelte';
  import { connection } from '../state/connection.svelte';
  import { versions } from '../state/versions.svelte';
  import { refreshToUpdate, retryConnection } from './reader';

  const update = $derived(versions.releasePending);
  const online = $derived(connection.online);

  // The stylesheet and the browser suite read the update state off `<html>`.
  $effect(() => {
    document.documentElement.dataset.updateState = update ? 'ready' : 'current';
  });
</script>

<div id="reader-announcement" class="sr-only" role="status" aria-live="polite">{announcer.message}</div>
<div id="pull-refresh" class="pull-refresh" role="status" hidden={pull.hidden}>
  <svg width="32" height="32" viewBox="0 0 32 32" fill="none" aria-hidden="true">
    <circle class="pull-refresh-ring" cx="16" cy="16" r="13" pathLength="100" stroke-width="1.5" />
  </svg>
  <svg class="pull-refresh-symbol" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d={pull.phase === 'refreshing' ? 'M20 11a8 8 0 1 0-2 6M20 4v7h-7' : 'M12 5v14m-5-5 5 5 5-5'} />
  </svg>
  <span class="sr-only">{pull.label}</span>
</div>
<div id="connection-status" class="connection-status" role="status" aria-live="polite" hidden={online && !update}>
  <span id="connection-status-message" hidden={update}>{online ? '' : 'Offline — downloaded articles remain available'}</span>
  <button id="connection-retry" type="button" hidden={online || update} onclick={retryConnection}>Retry</button>
  <button id="pwa-refresh" type="button" title="Refresh and keep your place" hidden={!update} onclick={refreshToUpdate}>↻ Refresh to update</button>
</div>
