<script lang="ts">
  // `base.html`'s status elements at the top of `#app`: the polite announcer, the
  // pull-to-refresh indicator, and the connection banner that carries the retry and the
  // "refresh to update" pill. Persistent chrome, outside any page scope; idle, it renders
  // exactly the static markup.
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
<div id="pull-refresh" class="pull-refresh" role="status" hidden={pull.hidden}>{pull.label}</div>
<div id="connection-status" class="connection-status" role="status" aria-live="polite" hidden={online && !update}>
  <span id="connection-status-message" hidden={update}>{online ? '' : 'Offline — downloaded articles remain available'}</span>
  <button id="connection-retry" type="button" hidden={online || update} onclick={retryConnection}>Retry</button>
  <button id="pwa-refresh" type="button" title="Refresh and keep your place" hidden={!update} onclick={refreshToUpdate}>↻ Refresh to update</button>
</div>
