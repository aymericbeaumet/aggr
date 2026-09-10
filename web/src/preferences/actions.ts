import type { PreferencesService } from "./service";

export function createPreferenceActions(root: ParentNode, service: PreferencesService) {
  let disposed = false;
  const select = <T extends HTMLElement>(selector: string) => root.querySelector<T>(selector);
  function fallback(url: string) {
    if (disposed) return;
    service.showLink(url);
    service.status("Copy the selected link. Clipboard access is unavailable.");
    queueMicrotask(() => {
      if (disposed) return;
      const field = select<HTMLInputElement>("#preferences-link");
      if (field) { field.hidden = false; field.value = url; field.focus(); field.select(); }
    });
  }
  return {
    shortcutURL: window.location.pathname + "#shortcut-help",
    canShare: typeof navigator.share === "function",
    async run(action: string) {
      if (disposed) return;
      if (action === "copy") {
        const url = service.link();
        try {
          if (!navigator.clipboard) { fallback(url); return; }
          await navigator.clipboard.writeText(url);
          if (!disposed) service.status("Preferences link copied.");
        } catch { fallback(url); }
      }
      if (action === "share") {
        try { await navigator.share({ title: "aggr preferences", url: service.link() }); }
        catch (error) { if (!disposed && !(error instanceof Error && error.name === "AbortError")) service.status("Sharing is unavailable. Use Copy link or Save file."); }
      }
      if (action === "save") {
        const url = URL.createObjectURL(new Blob([JSON.stringify(service.payload(), null, 2) + "\n"], { type: "application/json" }));
        const link = document.createElement("a");
        link.href = url; link.download = "aggr-preferences.json";
        document.body.appendChild(link); link.click(); link.remove();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
        service.status("Preferences file saved.");
      }
      if (action === "import") select<HTMLInputElement>("#preferences-file")?.click();
      if (action === "apply") service.applyImport();
      if (action === "cancel") service.cancelImport();
      if (action === "reset") service.reset();
    },
    dispose() { disposed = true; service.cancelFileRead(); },
  };
}
export type PreferenceActions = ReturnType<typeof createPreferenceActions>;
