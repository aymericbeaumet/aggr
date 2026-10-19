<script lang="ts">
  // The `#search-completions` listbox: empty and hidden on the static page, filled from the
  // state. The field keeps the keyboard (`aria-activedescendant` names the highlight); a pointer
  // chooses without blurring the field. Touch waits for release so the list can still scroll.
  import type { Completion } from './completion';
  import { search } from '../state/search.svelte';

  const shown = $derived(search.suggesting ? search.completions : []);
  let list: HTMLUListElement | undefined = $state();
  let maxHeight = $state<string | undefined>();
  let touch: { id: number; x: number; y: number; completion: Completion } | null = null;

  const detail = (completion: Completion) =>
    (completion.detail + (completion.count !== undefined ? ` · ${completion.count}` : '')).trim();

  function choose(completion: Completion): void {
    search.driver?.choose(completion);
  }

  function highlight(index: number): void {
    if (search.driver) search.driver.highlight(index);
    else search.highlighted = index;
  }

  function press(event: PointerEvent, completion: Completion): void {
    // Cancelling pointerdown keeps the field focused; native touch panning still delivers
    // pointercancel when the browser takes the gesture over.
    event.preventDefault();
    touch = null;
    if (event.pointerType !== 'touch') {
      choose(completion);
    } else if (event.isPrimary) {
      touch = { id: event.pointerId, x: event.clientX, y: event.clientY, completion };
    }
  }

  function move(event: PointerEvent, index: number): void {
    if (event.pointerType !== 'touch') return highlight(index);
    if (touch && (Math.abs(event.clientX - touch.x) > 10 || Math.abs(event.clientY - touch.y) > 10)) touch = null;
  }

  function release(event: PointerEvent): void {
    const started = touch;
    touch = null;
    if (started && event.pointerId === started.id &&
      Math.abs(event.clientX - started.x) <= 10 && Math.abs(event.clientY - started.y) <= 10) {
      choose(started.completion);
    }
  }

  // `vh` keeps the keyboard's space on mobile. Fit the list into what remains visible,
  // including the visual viewport's offset when the browser pans a focused input into view.
  $effect(() => {
    const node = list;
    if (!shown.length || !node) {
      maxHeight = undefined;
      return;
    }
    const viewport = window.visualViewport;
    const update = () => {
      const bottom = viewport ? viewport.offsetTop + viewport.height : window.innerHeight;
      const available = Math.max(0, Math.floor(bottom - node.getBoundingClientRect().top - 8));
      maxHeight = `min(24rem, ${available}px)`;
    };
    update();
    viewport?.addEventListener('resize', update, { passive: true });
    viewport?.addEventListener('scroll', update, { passive: true });
    window.addEventListener('resize', update, { passive: true });
    window.addEventListener('scroll', update, { passive: true });
    return () => {
      viewport?.removeEventListener('resize', update);
      viewport?.removeEventListener('scroll', update);
      window.removeEventListener('resize', update);
      window.removeEventListener('scroll', update);
    };
  });

  // A highlight that moved by keyboard stays in view of a long list.
  $effect(() => {
    const index = search.highlighted;
    if (!shown.length || !list) return;
    const item = list.children[index];
    if (item instanceof HTMLElement) item.scrollIntoView?.({ block: 'nearest' });
  });
</script>

<ul id="search-completions" class="search-completions" role="listbox" aria-label="Search suggestions" hidden={!shown.length} style:max-height={maxHeight} bind:this={list}>{#each shown as completion, index (completion.id)}<li class="search-completion" role="option" id="search-completion-{index}" data-completion-id={completion.id} aria-selected={index === search.highlighted} data-selected={index === search.highlighted ? '' : undefined} onpointerdown={(event) => press(event, completion)} onpointermove={(event) => move(event, index)} onpointerup={release} onpointercancel={() => (touch = null)}><span class="completion-label">{completion.label}</span>{#if detail(completion)}<small>{detail(completion)}</small>{/if}</li>{/each}</ul>
