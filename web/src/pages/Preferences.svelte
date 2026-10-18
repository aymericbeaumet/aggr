<script lang="ts">
  // `preferences.html`'s content block. The static page carries the heading and a note for
  // readers without JavaScript. The form is part of the app shell, so this tab paints its
  // controls in the same frame as the page instead of waiting on a chunk.
  import type { ClientPage } from '../generated/ClientPage';
  import type { ClientPreferences } from '../generated/ClientPreferences';
  import Form from '../preferences/Form.svelte';

  let { page, view }: { page: ClientPage; view: ClientPreferences } = $props();
</script>

<section class="page preferences" aria-labelledby="page-title">
  <header class="page-header"><h1 id="page-title">{page.title}</h1><p class="preferences-intro muted">Make yourself comfortable. Changes save automatically in this browser.</p></header>
  <div class="page-body preferences-body">
    <noscript><p>These settings are saved by your browser, so they need JavaScript. Reading and navigation work without it.</p></noscript>
    <Form schema={view.schema} />
  </div>
</section>
