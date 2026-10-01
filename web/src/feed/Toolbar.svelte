<script lang="ts">
  // `_feed_toolbar.html`: the complete search shell (the toolbar and the result container it
  // is followed by), rendered whole so search only fills it in. Without JavaScript the form
  // still submits to `?q=`, which lands on this feed. The server render is the static page's:
  // every binding starts at the value the template writes and diverges only as the state does.
  import { flushSync } from 'svelte';
  import type { ClientScope } from '../generated/ClientScope';
  import { urlFor } from '../model/urls';
  import { navigation } from '../navigation';
  import Completions, { completionKeydown } from '../search/Completions.svelte';
  import Results from '../search/Results.svelte';
  import { search } from '../state/search.svelte';
  import { selection } from '../state/selection.svelte';

  let { kind, scope, base }: { kind: string; scope: ClientScope | null; base: string } = $props();

  // A scoped list seeds the query with its own facet.
  const scoped = $derived(scope && scope.kind === kind ? scope : null);
  const seed = $derived(scoped ? `${kind}:${scoped.slug} ` : '');
  // The state takes the field over once its list has arrived; until then it holds the seed.
  const value = $derived(search.attached ? search.text : seed);
  const expanded = $derived(search.suggesting);

  const caret = (input: HTMLInputElement) => input.selectionStart ?? input.value.length;

  function input(event: Event): void {
    const target = event.currentTarget as HTMLInputElement;
    search.text = target.value;
    search.cursor = caret(target);
    search.drive((driver) => driver.changed());
  }

  function moved(event: Event): void {
    if (event instanceof KeyboardEvent && !['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    const target = event.currentTarget as HTMLInputElement;
    search.cursor = caret(target);
    search.driver?.moved();
  }

  /** The link of the row under the cursor, once the cursor has been drawn. */
  function selectedLink(): HTMLAnchorElement | null {
    const current = selection.current;
    if (!current) return null;
    flushSync();
    const row = Array.from(document.querySelectorAll<HTMLElement>('#list .row')).find((candidate) => candidate.dataset.path === current.path);
    const link = row?.querySelector('[data-row-open]');
    return link instanceof HTMLAnchorElement ? link : null;
  }

  /** Move the cursor over the results, keeping the keyboard in the field. */
  function step(direction: number): boolean {
    if (!search.ready || !selection.move(direction, search.address || location.href)) return false;
    const link = selectedLink();
    if (link) {
      link.closest('.row')?.scrollIntoView?.({ block: 'nearest', behavior: 'instant' });
      navigation.prefetch(link.href);
    }
    return true;
  }

  function keydown(event: KeyboardEvent): void {
    if (search.composing || event.isComposing) {
      event.stopPropagation();
      return;
    }
    if (completionKeydown(event)) return;
    // Without suggestions the arrows walk the results and Enter opens the selected one.
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (step(event.key === 'ArrowDown' ? 1 : -1)) {
        event.preventDefault();
        event.stopPropagation();
      }
      return;
    }
    if (event.key === 'Enter') {
      event.preventDefault();
      event.stopPropagation();
      search.drive((driver) => {
        driver.close();
        const link = search.ready ? selectedLink() : null;
        if (link) link.click();
        else driver.submit();
      });
    }
  }

  function focus(): void {
    search.focused = true;
    search.revealed = true;
    search.drive((driver) => driver.focus());
  }

  function blur(): void {
    search.focused = false;
    search.driver?.blur();
  }

  function composition(started: boolean, event: Event): void {
    search.composing = started;
    if (!started) input(event);
    else search.drive((driver) => driver.changed());
  }
</script>

<div class="feed-toolbar" data-nosnippet>
  <div data-search-root data-scope-kind={scoped ? kind : undefined} data-scope-value={scoped?.slug}>
    <div class="search-control" role="group" aria-label="Search controls">
      <div class="search-command">
        <form class="feed-search" id="search-form" action={urlFor(base, '')} method="get" role="search" onsubmit={(event) => { event.preventDefault(); search.drive((driver) => driver.submit()); }}>
          <label class="sr-only" for="q">Search articles</label>
          <div class="search-input-line">
            <!-- svelte-ignore a11y_role_has_required_aria_props -->
            <input id="q" type="search" name="q" {value} placeholder="Search articles, sources, categories…" autocomplete="off" spellcheck="false" role="combobox" aria-expanded={expanded ? 'true' : 'false'} aria-autocomplete="list" aria-activedescendant={expanded ? `search-completion-${search.highlighted}` : undefined} aria-describedby="search-query-help" data-page-search aria-keyshortcuts="Meta+K Control+K" oninput={input} onkeydown={keydown} onkeyup={moved} onclick={moved} onselect={moved} onfocus={focus} onblur={blur} oncompositionstart={(event) => composition(true, event)} oncompositionend={(event) => composition(false, event)}>
            <button type="button" class="search-clear" aria-label="Clear search" hidden={!search.clearable} onclick={() => search.drive((driver) => driver.clear())}>×</button>
          </div>
        </form>
        <Completions />
      </div>
      <div id="search-query-help" class="search-query-help" role="tooltip">
        <span>Use <code>source:</code>, <code>category:</code>, <code>tag:</code>, <code>type:</code>, <code>date:</code>, <code>sort:</code>, <code>"exact phrases"</code> or <code>-exclude</code>.</span>
      </div>
    </div>
    <p class="search-error" role="alert" hidden={!search.error}>{search.error}</p>
    <noscript><p class="search-offline">Full-text search needs JavaScript. Without it, browse by <a href={urlFor(base, 'browse/')}>category, source or tag</a>.</p></noscript>
  </div>
</div>
<Results {base} />
