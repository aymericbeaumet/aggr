<script lang="ts">
  import type { DiscussionLinkCtx } from '@aggr-model/DiscussionLinkCtx';
  let { discussions }: { discussions: DiscussionLinkCtx[] } = $props();
  const groups = [
    { id: 'scrolling', title: 'Scrolling', rows: [['d / Ctrl + d', 'Scroll down'], ['u / Ctrl + u', 'Scroll up'], ['Ctrl + e', 'One line down'], ['Ctrl + y', 'One line up'], ['g then g', 'Top of article'], ['G', 'Bottom of article']] },
    { id: 'articles', title: 'Articles', rows: [['j', 'Older article'], ['k', 'Newer article'], ['O', 'Open original']] },
    { id: 'lists', title: 'Lists', rows: [['j / ↓', 'Next item'], ['k / ↑', 'Previous item'], ['g then g', 'First item'], ['G', 'Last item'], ['o / Enter', 'Open selected item']] },
    { id: 'navigation', title: 'Go to', rows: [['g then f / g then i', 'Feed'], ['g then 1–9', 'Open feed entry'], ['g then b', 'Browse'], ['g then p', 'Preferences']] },
    { id: 'anywhere', title: 'Search and help', rows: [['⌘ + k / Ctrl + k / /', 'Global search'], ['?', 'Show this help'], ['Esc', 'Close this help']] },
  ];
</script>
{#snippet key(token: string)}
  {#if token.includes(' + ') || token.includes(' then ')}
    {@const separator = token.includes(' + ') ? '+' : 'then'}
    {@const pair = token.split(separator === '+' ? ' + ' : ' then ')}
    <span class="key-pair" data-platform-key={token.startsWith('⌘') ? 'apple' : token === 'Ctrl + k' ? 'other' : undefined}><kbd>{pair[0]}</kbd><small>{separator}</small><kbd>{pair[1]}</kbd></span>
  {:else}<kbd>{token}</kbd>{/if}
{/snippet}
<dialog id="shortcut-help" class="shortcut-help" aria-labelledby="shortcut-help-title" aria-keyshortcuts="?" data-nosnippet>
  <form method="dialog"><header class="shortcut-help-head"><h2 id="shortcut-help-title" tabindex="-1">Keyboard shortcuts</h2><button class="shortcut-close" value="close" aria-label="Close keyboard shortcuts">×</button></header>
  <div class="shortcut-groups">{#each groups as group}<section aria-labelledby="shortcut-{group.id}"><h3 id="shortcut-{group.id}">{group.title}</h3><dl class="shortcut-list">{#each group.rows as [keys, label]}<div><dt class={keys.includes(' / ') ? 'key-alternatives' : undefined}>{#if keys.startsWith('⌘')}{@render key('⌘ + k')}{@render key('Ctrl + k')}<small>or</small><kbd>/</kbd>{:else}{#each keys.split(' / ') as token, index}{#if index}<small>or</small>{/if}{@render key(token)}{/each}{/if}</dt><dd>{label}</dd></div>{/each}{#if group.id === 'articles'}{#each discussions.filter((item) => item.shortcut) as discussion}<div><dt><kbd>{discussion.shortcut}</kbd></dt><dd>Open {discussion.name} discussion / search</dd></div>{/each}{/if}</dl></section>{/each}</div></form>
</dialog>
