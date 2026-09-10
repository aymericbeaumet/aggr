<script lang="ts">
  import { dateLabel, dateTooltip } from './display';

  let { published, updated, dateFormat, now = Date.now() }: {
    published: string; updated?: string; dateFormat: unknown; now?: number;
  } = $props();
  let localized = $state(false);
  const label = $derived(dateLabel(published, dateFormat, now));
  const changed = $derived(updated && Number.isFinite(Date.parse(updated)) && Date.parse(updated) !== Date.parse(published) ? updated : undefined);
  const tooltip = $derived(dateTooltip(published, changed, localized));
  function localizeOnHover(node: HTMLElement) {
    const localize = () => { localized = true; };
    node.addEventListener('pointerenter', localize);
    node.addEventListener('focusin', localize);
    return { destroy() {
      node.removeEventListener('pointerenter', localize);
      node.removeEventListener('focusin', localize);
    } };
  }
</script>

<span class="published-date" data-date-tooltip data-date-updated={changed} title={tooltip}
  use:localizeOnHover>
  <time class="dt-published" datetime={published} aria-label={`${label}; ${tooltip}`}>{label}</time>
</span>
