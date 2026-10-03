<script lang="ts">
  // The thumbnail of a feed row or related card: `_item.html` and `_related_item.html` share
  // this markup. A missing colour sets no custom property.
  import type { ClientPreview } from '../generated/ClientPreview';
  import { urlFor } from '../model/urls';

  let { preview, base }: { preview: ClientPreview; base: string } = $props();
</script>

<span class="preview-media" style="{preview.color ? `--preview-color: ${preview.color}; ` : ''}--image-preview: url('{preview.placeholder.data_url}')"><img class="preview-image" src={urlFor(base, preview.url)} width={preview.width} height={preview.height} alt={preview.alt ?? ''} loading="lazy" decoding="async"></span>
