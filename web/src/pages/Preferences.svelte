<script lang="ts">
  import type { ClientPage } from '@aggr-model/ClientPage';
  import type { ClientPreferences } from '@aggr-model/ClientPreferences';

  let { page, view }: { page: ClientPage; view: ClientPreferences } = $props();

  import Form from '../preferences/Form.svelte';
</script>

<section class="page preferences" aria-labelledby="page-title">
  <header class="page-header"><h1 id="page-title">{page.title}</h1><p class="preferences-intro muted">Make yourself comfortable. Changes save automatically in this browser.</p></header>
  <div class="page-body preferences-body">
    <noscript><p>These settings are saved by your browser, so they need JavaScript. Reading and navigation work without it.</p></noscript>
    <Form schema={view.schema} />
  </div>
</section>
