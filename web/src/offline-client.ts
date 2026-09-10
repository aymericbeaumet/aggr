import type { BuildManifest } from "./contracts";
import type { OfflineStatus } from "./offline";

interface OfflineClientOptions {
  base: () => string;
  pwa: () => boolean;
  kind: () => string;
  count: () => number;
  onBuild: (build: BuildManifest) => void;
  onStatus: (status: OfflineStatus) => void;
  onControllerChange: () => void;
}
interface OfflineEnvironment { window: Window; document: Document; navigator: Navigator }

export function createOfflineClient(options: OfflineClientOptions, environment: OfflineEnvironment = { window, document, navigator }) {
  const { window: win, document: doc, navigator: nav } = environment;
  let phase: "idle" | "started" | "disposed" = "idle";
  let configuredController: ServiceWorker | null = null;
  let configuredCount: number | null = null;
  let updateTimer: ReturnType<typeof setInterval> | undefined;
  const removeListeners: Array<() => void> = [];
  const active = () => phase !== "disposed";
  function listen(target: EventTarget, name: string, listener: EventListener) {
    target.addEventListener(name, listener);
    removeListeners.push(() => target.removeEventListener(name, listener));
  }
  function configure(force = false) {
    if (!active() || !options.pwa() || !("serviceWorker" in nav)) return;
    const controller = nav.serviceWorker.controller;
    const count = options.count();
    if (!controller || !Number.isInteger(count)) return;
    if (!force && configuredController === controller && configuredCount === count) return;
    configuredController = controller;
    configuredCount = count;
    controller.postMessage({ type: "AGGR_OFFLINE_CONFIG", count });
  }
  function start() {
    if (phase !== "idle") return;
    phase = "started";
    if (!("serviceWorker" in nav) || win.location.protocol === "file:") return;
    const workers = nav.serviceWorker;
    const base = options.base();
    if (!options.pwa()) {
      void workers.getRegistration(base).then(registration => {
        if (active() && registration?.scope === new URL(base).href) return registration.unregister();
      }).catch(() => {});
      if ("caches" in win) {
        const namespace = "aggr:" + encodeURIComponent(new URL(base).pathname) + ":";
        void win.caches.keys().then(names => {
          if (!active()) return;
          return Promise.all(names.filter(name => name.startsWith(namespace)).map(name => win.caches.delete(name)));
        }).catch(() => {});
      }
      return;
    }
    if (options.kind() === "html") return;

    listen(workers, "message", event => {
      const message = event as MessageEvent;
      if (!workers.controller || message.source !== workers.controller || !message.data || typeof message.data !== "object") return;
      if (message.data.type === "AGGR_BUILD") options.onBuild(message.data);
      else if (message.data.type === "AGGR_OFFLINE_STATUS") options.onStatus(message.data);
    });
    function requestStatus() {
      const controller = workers.controller;
      if (!active() || !controller) return;
      controller.postMessage({ type: "AGGR_GET_BUILD" });
      controller.postMessage({ type: "AGGR_OFFLINE_GET_STATUS" });
    }
    void workers.ready.then(() => {
      if (!active()) return;
      configure(); requestStatus();
    }).catch(() => {});
    listen(win, "online", () => configure(true));
    let controller = workers.controller;
    listen(workers, "controllerchange", () => {
      const next = workers.controller;
      if (controller && next && controller !== next) options.onControllerChange();
      controller = next;
      requestStatus(); configure(true);
    });
    void workers.register(new URL("sw.js", base).href, { updateViaCache: "none" }).then(registration => {
      if (!active()) return;
      const interval = 60000;
      let lastCheck = Date.now();
      let checking = false;
      function check() {
        if (!active() || checking || !nav.onLine || doc.visibilityState !== "visible") return;
        checking = true;
        lastCheck = Date.now();
        void registration.update().catch(() => {}).finally(() => { checking = false; });
      }
      updateTimer = setInterval(check, interval);
      listen(doc, "visibilitychange", () => { if (doc.visibilityState === "visible") check(); });
      listen(win, "online", check);
      listen(win, "pageshow", event => {
        if ((event as PageTransitionEvent).persisted || Date.now() - lastCheck >= interval) check();
      });
    }).catch(() => {});
  }
  function dispose() {
    if (phase === "disposed") return;
    phase = "disposed";
    clearInterval(updateTimer);
    removeListeners.splice(0).forEach(remove => remove());
    configuredController = null;
    configuredCount = null;
  }
  return { configure, start, dispose };
}
