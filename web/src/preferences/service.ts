import type { Preferences, PreferenceValues } from "../contracts";

export interface PreferenceSnapshot {
  values: PreferenceValues;
  pending: PreferenceValues | null;
  status: string;
  link: string;
  offlineSummary: string;
}
export interface PreferenceEnvironment {
  base: () => string;
  document: Document;
  window: Window;
  onApply: (persist: boolean) => void;
  replaceLocation: (url: string) => void;
}

export function createPreferencesService(bootstrap: Preferences, env: PreferenceEnvironment) {
  let snapshot: PreferenceSnapshot = { values: { ...bootstrap.values }, pending: null, status: "", link: "", offlineSummary: "" };
  const listeners = new Set<(state: PreferenceSnapshot) => void>();
  let importGeneration = 0;
  let disposed = false;
  let themeInitialized = false;
  let appliedTheme: string | undefined;
  function publish(patch: Partial<PreferenceSnapshot>) {
    if (disposed) return;
    snapshot = { ...snapshot, ...patch };
    listeners.forEach(listener => listener(snapshot));
  }
  function status(message: string) { publish({ status: message }); }
  function refreshTheme() {
    if (disposed) return;
    const meta = env.document.querySelector("#theme-color");
    if (meta) {
      meta.setAttribute("content", env.window.getComputedStyle(env.document.documentElement).getPropertyValue("--nav-bg").trim());
      appliedTheme = env.document.documentElement.dataset.theme;
      themeInitialized = true;
    }
  }
  function apply(values: PreferenceValues, persist = true) {
    if (disposed) return;
    let saved = true;
    let changed = false;
    for (const [key, value] of Object.entries(values)) {
      if (!bootstrap.valid(key, value)) continue;
      changed ||= snapshot.values[key] !== value;
      bootstrap.values[key] = value;
      if (persist) {
        try { env.window.localStorage.setItem("aggr:" + key, String(value)); } catch { saved = false; }
      }
      const attribute = bootstrap.schema[key].attribute;
      if (attribute && env.document.documentElement.dataset[attribute] !== String(value)) {
        env.document.documentElement.dataset[attribute] = String(value);
      }
    }
    if (changed || persist) {
      publish({ ...(changed ? { values: { ...bootstrap.values } } : {}), ...(persist ? { status: saved ? "Saved in this browser." : "Applied for this session; browser storage is unavailable." } : {}) });
    }
    if (!themeInitialized || appliedTheme !== env.document.documentElement.dataset.theme) refreshTheme();
    env.onApply(persist);
    return saved;
  }
  function parse(raw: string) {
    if (raw.length > 16384) throw new Error("Preferences file is too large");
    return bootstrap.validate(JSON.parse(raw));
  }
  function payload() { return { version: 1, preferences: { ...bootstrap.values } }; }
  function link() {
    const encoded = env.window.btoa(JSON.stringify(payload())).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    const url = new URL("preferences/", env.base());
    url.hash = "aggr-state=" + encoded;
    return url.href;
  }
  function importLocation(kind: string) {
    const url = new URL(env.window.location.href);
    const fragment = new URLSearchParams(url.hash.slice(1));
    const encoded = fragment.get("aggr-state");
    if (!encoded) return;
    importGeneration++;
    url.hash = "";
    env.replaceLocation(url.href);
    try {
      if (encoded.length > 22000 || !/^[A-Za-z0-9_-]+={0,2}$/.test(encoded)) throw new Error("Invalid preferences link");
      let base64 = encoded.replace(/-/g, "+").replace(/_/g, "/");
      while (base64.length % 4) base64 += "=";
      publish({ pending: parse(env.window.atob(base64)), status: "Review the imported settings before applying them." });
      if (kind !== "preferences") {
        const destination = new URL("preferences/", env.base());
        destination.hash = "aggr-state=" + encoded;
        env.window.location.replace(destination.href);
      }
    } catch {
      publish({ pending: null, status: "This preferences link is invalid or unsupported. Nothing was changed." });
    }
  }
  async function importFile(file: Pick<File, "size" | "text">) {
    const generation = ++importGeneration;
    try {
      if (file.size > 16384) throw new Error("File too large");
      const raw = await file.text();
      if (disposed || generation !== importGeneration) return;
      publish({ pending: parse(raw), status: "Review the imported settings before applying them." });
    } catch {
      if (disposed || generation !== importGeneration) return;
      publish({ pending: null, status: "This file is invalid or unsupported. Use an aggr preferences JSON file (up to 16 KB). Nothing was changed." });
    }
  }
  function cancelFileRead() { importGeneration++; }
  function readStorage() { apply(bootstrap.read(), false); }
  function storage(event: StorageEvent) {
    if (event.key === null || Object.keys(bootstrap.schema).some(key => event.key === "aggr:" + key)) readStorage();
  }
  env.window.addEventListener("storage", storage);
  return {
    subscribe(listener: (state: PreferenceSnapshot) => void) { listeners.add(listener); listener(snapshot); return () => { listeners.delete(listener); }; },
    getSnapshot: () => snapshot,
    apply, readStorage, refreshTheme, importLocation, importFile, cancelFileRead, payload, link, status,
    setOfflineSummary(offlineSummary: string) { if (snapshot.offlineSummary !== offlineSummary) publish({ offlineSummary }); },
    showLink(link: string) { publish({ link }); },
    applyImport() { cancelFileRead(); if (snapshot.pending) { apply(snapshot.pending); publish({ pending: null }); } },
    cancelImport() { cancelFileRead(); publish({ pending: null, status: "Import cancelled. Your preferences were not changed." }); },
    reset() {
      cancelFileRead();
      const saved = apply(Object.fromEntries(Object.entries(bootstrap.schema).map(([key, rule]) => [key, rule.initial])));
      publish({ pending: null, status: saved ? "Default preferences restored." : "Defaults applied for this session; browser storage is unavailable." });
    },
    dispose() { disposed = true; cancelFileRead(); listeners.clear(); env.window.removeEventListener("storage", storage); },
  };
}

export type PreferencesService = ReturnType<typeof createPreferencesService>;
