<script lang="ts">
  import { placeholderBackground, type Display } from './display';
  let { preview, src }: { preview: NonNullable<Display['preview']>; src: string } = $props();
  let image: HTMLImageElement | undefined = $state();
  let loaded = $state(false);
  let failed = $state(false);
  $effect(() => {
    src;
    loaded = !!image?.complete && image.naturalWidth > 0;
    failed = !!image?.complete && image.naturalWidth === 0 && !!image.currentSrc;
  });
</script>

<span class="preview-media" class:is-loaded={loaded} class:is-error={failed} data-preview-bound="true" data-thumbhash={preview.placeholder?.hash}
  style:--image-preview={loaded ? undefined : placeholderBackground(preview.placeholder?.data_url)}
  style:--preview-color={/^#[0-9a-f]{6}$/i.test(preview.color || '') ? preview.color : undefined}>
  <img bind:this={image} class="preview-image" {src} width={preview.width} height={preview.height} alt={preview.alt || ''}
    loading="lazy" decoding="async" onload={() => { loaded = true; failed = false; }} onerror={() => { loaded = false; failed = true; }} />
</span>
