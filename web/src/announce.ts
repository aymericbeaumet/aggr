// The reader speaks through one always-present polite live region. Toggling `hidden` on a live
// region, or mounting one with its text already inside, is not reliably announced, so status text
// is written here instead. Clearing first and writing on the next frame makes a repeated message
// count as a new change.
export function announce(text: string, root: Document = document) {
  const announcer = root.querySelector<HTMLElement>("#aggr-announcer");
  if (!announcer) return;
  announcer.textContent = "";
  requestAnimationFrame(() => { announcer.textContent = text; });
}
