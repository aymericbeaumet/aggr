/** The keyboard help is the static `<dialog id="shortcut-help">` outside the app. */
function dialog(): HTMLDialogElement | null {
  const element = document.getElementById('shortcut-help');
  return element instanceof HTMLDialogElement ? element : null;
}

export function openShortcutHelp(): void {
  const help = dialog();
  if (help && !help.open) help.showModal();
}

export function installShortcutHelp(): void {
  const help = dialog();
  if (!help) return;
  // Where invoker commands are unsupported, wire the button up by hand.
  if (!('command' in HTMLButtonElement.prototype)) {
    for (const button of document.querySelectorAll("[commandfor='shortcut-help']")) {
      button.addEventListener('click', (event) => {
        event.preventDefault();
        openShortcutHelp();
      });
    }
  }
  // A click on the backdrop lands on the dialog itself.
  help.addEventListener('click', (event) => {
    if (event.target === help) help.close();
  });
}
