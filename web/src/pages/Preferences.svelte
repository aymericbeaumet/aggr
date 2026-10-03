<script lang="ts">
  // `preferences.html`'s content block. The static page carries the heading and a note for
  // readers without JavaScript; the form exists only in the client and is loaded with the page,
  // so the entry bundle carries none of it.
  import type { ClientPage } from '../generated/ClientPage';
  import type { ClientPreferences } from '../generated/ClientPreferences';

  let { page, view }: { page: ClientPage; view: ClientPreferences } = $props();

  const form = import('../preferences/Form.svelte');
</script>

<section class="page preferences" aria-labelledby="page-title">
  <header class="page-header"><h1 id="page-title">{page.title}</h1><p class="preferences-intro muted">Make yourself comfortable. Changes save automatically in this browser.</p></header>
  <div class="page-body preferences-body">
    <noscript><p>These settings are saved by your browser, so they need JavaScript. Reading and navigation work without it.</p></noscript>
    {#await form then { default: Form }}<Form schema={view.schema} />{/await}
  </div>
</section>
