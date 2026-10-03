<script lang="ts">
  // The Share action floating above a selected passage. It lives on `document.body`, outside the
  // page's own markup, and is shown at a box the page hands it: it never reads the selection.
  let { link }: { link: () => string } = $props();

  let element = $state<HTMLDivElement | null>(null);
  let anchor = $state<{ left: number; top: number; width: number; height: number } | null>(null);
  let left = $state(0);
  let top = $state(0);
  let label = $state('Share');
  let timer: ReturnType<typeof setTimeout> | undefined;

  const hidden = $derived(anchor === null);

  // Measured once shown: centred over the selection, kept inside the viewport, above the box.
  $effect(() => {
    const box = anchor;
    if (!box || !element) return;
    const width = element.offsetWidth;
    const height = element.offsetHeight;
    left = Math.max(8, Math.min(window.innerWidth - width - 8, box.left + box.width / 2 - width / 2)) + window.scrollX;
    top = Math.max(0, box.top + window.scrollY - height - 10);
  });

  export function show(box: { left: number; top: number; width: number; height: number }): void {
    anchor = { left: box.left, top: box.top, width: box.width, height: box.height };
  }

  export function hide(): void {
    anchor = null;
  }

  export function shown(): boolean {
    return anchor !== null;
  }

  async function share(): Promise<void> {
    const url = link();
    try {
      const native = typeof navigator.share === 'function';
      if (native) await navigator.share({ title: document.title, url });
      else await navigator.clipboard.writeText(url);
      label = native ? 'Shared' : 'Copied';
    } catch (error) {
      if (error instanceof DOMException && error.name === 'AbortError') return;
      label = 'Copy failed';
    }
    clearTimeout(timer);
    timer = setTimeout(() => (label = 'Share'), 1600);
  }

  $effect(() => () => clearTimeout(timer));
</script>

<div class="selection-share" bind:this={element} {hidden} style="left: {left}px; top: {top}px">
  <button type="button" onpointerdown={(event) => event.preventDefault()} onclick={() => void share()}>{label}</button>
</div>
