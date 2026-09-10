import { writable } from 'svelte/store';
import type { AppContext, BuildManifest } from './contracts';
import type { OfflineStatus } from './offline';

export interface UpdateSnapshot {
  phase: 'current' | 'ready' | 'reloading';
  appVersion?: string;
  availableAppVersion?: string;
  contentVersion?: string;
  online: boolean;
  offline: OfflineStatus | null;
  notice: { message: string; retry: boolean } | null;
}

export function connectionPresentation(state: UpdateSnapshot) {
  if (state.notice) return { visible: true, ...state.notice, refresh: false, disabled: false };
  if (!state.online) return { visible: true, message: 'Offline — showing saved pages.', retry: true, refresh: false, disabled: false };
  const refresh = state.phase !== 'current';
  return { visible: refresh, message: '', retry: false, refresh, disabled: state.phase === 'reloading' };
}

export function createUpdates(initial: AppContext, online: boolean) {
  let state: UpdateSnapshot = { phase: 'current', appVersion: initial.appVersion,
    availableAppVersion: initial.appVersion, contentVersion: initial.contentVersion,
    online, offline: null, notice: null };
  const store = writable(state);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const update = (change: Partial<UpdateSnapshot>) => { state = { ...state, ...change }; store.set(state); };
  return {
    subscribe: store.subscribe,
    snapshot: () => state,
    receive(build: BuildManifest | null) {
      if (!build || typeof build.app_version !== 'string' || !build.app_version
        || typeof build.content_version !== 'string' || !build.content_version) return { appChanged: false, contentChanged: false };
      const appChanged = !!state.appVersion && build.app_version !== state.appVersion && build.app_version !== state.availableAppVersion;
      const contentChanged = build.content_version !== state.contentVersion;
      update({ availableAppVersion: build.app_version, contentVersion: build.content_version,
        ...(appChanged ? { phase: state.phase === 'reloading' ? 'reloading' : 'ready', notice: null } : {}) });
      return { appChanged, contentChanged };
    },
    setOnline(online: boolean) { clearTimeout(timer); update({ online, notice: null }); },
    setOffline(offline: OfflineStatus) { update({ offline }); },
    notice(message: string, retry = false, temporary = false) {
      clearTimeout(timer);
      update({ notice: { message, retry } });
      if (temporary) timer = setTimeout(() => update({ notice: null }), 2200);
    },
    beginReload() {
      if (state.phase !== 'ready') return false;
      update({ phase: 'reloading' });
      return true;
    },
    dispose() { clearTimeout(timer); }
  };
}
export type Updates = ReturnType<typeof createUpdates>;
