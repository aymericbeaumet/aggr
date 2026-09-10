export function interactiveUrl(value: string | undefined): URL | undefined {
  if (!value) return;
  try {
    const url = new URL(value);
    if (['http:', 'https:'].includes(url.protocol) && url.hostname && !url.username && !url.password) return url;
  } catch { /* Invalid stored metadata keeps its ordinary link fallback. */ }
}

export function enhanceInteractive(root: ParentNode, signal: AbortSignal) {
  if (signal.aborted) return;
  for (const link of root.querySelectorAll<HTMLAnchorElement>('[data-interactive-embed]')) {
    const container = link.closest<HTMLElement>('.interactive-frame');
    const url = interactiveUrl(link.dataset.interactiveEmbed);
    if (!container || !url || link.dataset.interactiveBound || container.querySelector('iframe')) continue;
    link.dataset.interactiveBound = 'true';
    const frame = document.createElement('iframe');
    frame.title = link.getAttribute('aria-label') || 'Interactive page';
    frame.setAttribute('sandbox', 'allow-scripts');
    frame.referrerPolicy = 'no-referrer';
    frame.src = url.href;
    container.setAttribute('aria-busy', 'true');
    frame.addEventListener('load', () => {
      container.removeAttribute('aria-busy');
      link.hidden = true;
    }, { once: true, signal });
    signal.addEventListener('abort', () => {
      frame.remove();
      container.removeAttribute('aria-busy');
      link.hidden = false;
      delete link.dataset.interactiveBound;
    }, { once: true });
    container.appendChild(frame);
  }
}
