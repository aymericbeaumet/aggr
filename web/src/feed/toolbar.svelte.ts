import { flushSync, mount, unmount } from 'svelte';
import { navigation } from '../navigation';
import Completions from '../search/Completions.svelte';
import { completionKeydown } from '../search/completion-keys';
import Results from '../search/Results.svelte';
import { page } from '../state/page.svelte';
import { search } from '../state/search.svelte';
import { selection } from '../state/selection.svelte';

/** The search form stays Rust HTML; only suggestions and results are client-only islands. */
export function installToolbar(root: HTMLElement): () => void {
  const field = root.querySelector<HTMLInputElement>('#q');
  if (!field) return () => {};
  const controller = new AbortController();
  const options = { signal: controller.signal };
  const instances: ReturnType<typeof mount>[] = [];
  const target = (selector: string, fallback: string): HTMLElement | null => {
    const existing = root.querySelector<HTMLElement>(selector);
    if (existing) return existing;
    const old = root.querySelector(fallback);
    if (!old) return null;
    const holder = document.createElement('div');
    holder.style.display = 'contents';
    old.replaceWith(holder);
    return holder;
  };
  const completions = target('[data-search-completions-root]', '#search-completions');
  const results = target('[data-search-results-root]', '[data-search-results]');
  if (completions) instances.push(mount(Completions, { target: completions }));
  if (results) instances.push(mount(Results, { target: results, props: { base: page.model?.base ?? './' } }));

  const caret = () => field.selectionStart ?? field.value.length;
  const input = () => {
    search.text = field.value;
    search.cursor = caret();
    search.drive((driver) => driver.changed());
  };
  const moved = (event: Event) => {
    if (event instanceof KeyboardEvent && !['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    search.cursor = caret();
    search.driver?.moved();
  };
  const selectedLink = (): HTMLAnchorElement | null => {
    const current = selection.current;
    if (!current) return null;
    flushSync();
    const row = [...root.querySelectorAll<HTMLElement>('#list .row')].find((candidate) => candidate.dataset.path === current.path);
    return row?.querySelector<HTMLAnchorElement>('[data-row-open]') ?? null;
  };
  const step = (direction: number): boolean => {
    if (!search.ready || !selection.move(direction, search.address || location.href)) return false;
    const link = selectedLink();
    if (link) {
      link.closest('.row')?.scrollIntoView?.({ block: 'nearest', behavior: 'instant' });
      navigation.prefetch(link.href);
    }
    return true;
  };
  field.addEventListener('input', input, options);
  field.addEventListener('change', () => { if (field.value !== search.text) input(); }, options);
  for (const name of ['keyup', 'click', 'select']) field.addEventListener(name, moved, options);
  field.addEventListener('keydown', (event) => {
    if (search.composing || event.isComposing) { event.stopPropagation(); return; }
    if (completionKeydown(event)) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (step(event.key === 'ArrowDown' ? 1 : -1)) { event.preventDefault(); event.stopPropagation(); }
    } else if (event.key === 'Enter') {
      event.preventDefault(); event.stopPropagation();
      search.drive((driver) => {
        driver.close();
        const link = search.ready ? selectedLink() : null;
        if (link) link.click(); else driver.submit();
      });
    }
  }, options);
  field.addEventListener('focus', () => {
    search.focused = true; search.revealed = true; search.drive((driver) => driver.focus());
  }, options);
  field.addEventListener('blur', () => {
    search.focused = false; search.composing = false; search.driver?.blur();
  }, options);
  field.addEventListener('compositionstart', () => { search.composing = true; search.drive((driver) => driver.changed()); }, options);
  field.addEventListener('compositionend', () => { search.composing = false; input(); }, options);
  root.querySelector('#search-form')?.addEventListener('submit', (event) => { event.preventDefault(); search.drive((driver) => driver.submit()); }, options);
  root.querySelector('.search-clear')?.addEventListener('click', () => search.drive((driver) => driver.clear()), options);
  const stop = $effect.root(() => {
    $effect(() => {
      if (search.attached && field.value !== search.text) field.value = search.text;
    });
    $effect(() => {
      field.setAttribute('aria-expanded', String(search.suggesting));
      if (search.suggesting) field.setAttribute('aria-activedescendant', `search-completion-${search.highlighted}`);
      else field.removeAttribute('aria-activedescendant');
      const clear = root.querySelector<HTMLButtonElement>('.search-clear');
      if (clear) clear.hidden = !search.clearable;
      const error = root.querySelector<HTMLElement>('.search-error');
      if (error) { error.hidden = !search.error; error.textContent = search.error; }
    });
  });
  return () => {
    controller.abort(); stop();
    for (const instance of instances) void unmount(instance);
  };
}
