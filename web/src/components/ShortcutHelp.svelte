<script lang="ts">
  import type { AppContext } from '../contracts';
  let { discussions = [] }: { discussions?: AppContext['discussions'] } = $props();
</script>

{#snippet keyPair(first: string, second: string, separator = '+')}
  <span class="key-pair"><kbd>{first}</kbd><small>{separator}</small><kbd>{second}</kbd></span>
{/snippet}

<dialog id="shortcut-help" class="shortcut-help" aria-labelledby="shortcut-help-title" aria-keyshortcuts="?" data-nosnippet>
  <form method="dialog">
    <header class="shortcut-help-head">
      <h2 id="shortcut-help-title" tabindex="-1">Keyboard shortcuts</h2>
      <button class="shortcut-close" value="close" aria-label="Close keyboard shortcuts">×</button>
    </header>
    <div class="shortcut-groups">
      <section aria-labelledby="shortcut-scrolling">
        <h3 id="shortcut-scrolling">Scrolling</h3>
        <dl class="shortcut-list">
          <div><dt class="key-alternatives"><kbd>d</kbd><small>or</small>{@render keyPair('Ctrl', 'd')}</dt><dd>Scroll down</dd></div>
          <div><dt class="key-alternatives"><kbd>u</kbd><small>or</small>{@render keyPair('Ctrl', 'u')}</dt><dd>Scroll up</dd></div>
          <div><dt>{@render keyPair('Ctrl', 'e')}</dt><dd>One line down</dd></div>
          <div><dt>{@render keyPair('Ctrl', 'y')}</dt><dd>One line up</dd></div>
          <div><dt>{@render keyPair('g', 'g', 'then')}</dt><dd>Top of article</dd></div>
          <div><dt><kbd>G</kbd></dt><dd>Bottom of article</dd></div>
        </dl>
      </section>
      <section aria-labelledby="shortcut-articles">
        <h3 id="shortcut-articles">Articles</h3>
        <dl class="shortcut-list">
          <div><dt><kbd>j</kbd></dt><dd>Older article</dd></div>
          <div><dt><kbd>k</kbd></dt><dd>Newer article</dd></div>
          <div><dt><kbd>O</kbd></dt><dd>Open original</dd></div>
          {#each discussions as discussion}{#if discussion.shortcut}<div><dt><kbd>{discussion.shortcut}</kbd></dt><dd>Open {discussion.name} discussion / search</dd></div>{/if}{/each}
        </dl>
      </section>
      <section aria-labelledby="shortcut-lists">
        <h3 id="shortcut-lists">Lists</h3>
        <dl class="shortcut-list">
          <div><dt><kbd>j</kbd></dt><dd>Next item</dd></div>
          <div><dt><kbd>k</kbd></dt><dd>Previous item</dd></div>
          <div><dt>{@render keyPair('g', 'g', 'then')}</dt><dd>First item</dd></div>
          <div><dt><kbd>G</kbd></dt><dd>Last item</dd></div>
          <div><dt class="key-alternatives"><kbd>o</kbd><small>or</small><kbd>Enter</kbd></dt><dd>Open selected item</dd></div>
        </dl>
      </section>
      <section aria-labelledby="shortcut-navigation">
        <h3 id="shortcut-navigation">Go to</h3>
        <dl class="shortcut-list">
          <div><dt class="key-alternatives">{@render keyPair('g', 'f', 'then')}<small>or</small>{@render keyPair('g', 'i', 'then')}</dt><dd>Feed</dd></div>
          <div><dt>{@render keyPair('g', '1–9', 'then')}</dt><dd>Open feed entry</dd></div>
          <div><dt>{@render keyPair('g', 'l', 'then')}</dt><dd>Browse</dd></div>
          <div><dt>{@render keyPair('g', 'p', 'then')}</dt><dd>Preferences</dd></div>
        </dl>
      </section>
      <section aria-labelledby="shortcut-anywhere">
        <h3 id="shortcut-anywhere">Search and help</h3>
        <dl class="shortcut-list">
          <div><dt class="key-alternatives"><kbd>/</kbd><small>or</small>{@render keyPair('⌘', 'k')}<small>or</small>{@render keyPair('Ctrl', 'k')}</dt><dd>Global search</dd></div>
          <div><dt><kbd>?</kbd></dt><dd>Show this help</dd></div>
          <div><dt><kbd>Esc</kbd></dt><dd>Close this help</dd></div>
        </dl>
      </section>
    </div>
  </form>
</dialog>
