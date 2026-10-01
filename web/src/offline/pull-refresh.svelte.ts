/**
 * Pull-to-refresh for the installed app, where the browser's own is out of reach: a single
 * touch dragged down from the top of the page pulls the header and the content along, and a
 * pull far enough reloads the page keeping the reader's place. The indicator is `#pull-refresh`
 * in the status bar; the stylesheet moves the page through `data-pull-state` on `<html>`.
 */

export type PullPhase = 'idle' | 'tracking' | 'pulling' | 'armed' | 'refreshing';

const TRACKING: readonly string[] = ['tracking', 'pulling', 'armed'];

/** Where a touch that started at the top leaves the gesture, from how far it moved. */
export function pullState(phase: string, dx: number, dy: number): { phase: string; distance: number } {
  if (!TRACKING.includes(phase)) return { phase, distance: 0 };
  if (dy <= 0 || Math.abs(dx) > dy) return { phase: 'idle', distance: 0 };
  if (dy < 6) return { phase, distance: 0 };
  return { phase: dy >= 84 ? 'armed' : 'pulling', distance: Math.min(72, Math.round(dy * 0.55)) };
}

/** What the indicator says in each phase; nothing while there is nothing to show. */
export function pullLabel(phase: string): string {
  switch (phase) {
    case 'armed':
      return 'Release to refresh';
    case 'refreshing':
      return 'Refreshing…';
    case 'pulling':
      return 'Pull to refresh';
    default:
      return '';
  }
}

class Pull {
  phase = $state<PullPhase>('idle');
  distance = $state(0);

  get hidden(): boolean {
    return this.phase === 'idle' || this.phase === 'tracking';
  }

  get label(): string {
    return pullLabel(this.phase);
  }
}

export const pull = new Pull();

/** Whether the page runs as an installed app rather than in a browser tab. */
export function installedApp(win: Window = window): boolean {
  return (
    win.matchMedia('(display-mode: standalone), (display-mode: minimal-ui), (display-mode: fullscreen)').matches ||
    win.navigator.standalone === true
  );
}

export type PullOptions = {
  /** Whether the gesture applies at all: the site has a worker and runs installed. */
  enabled: () => boolean;
  refresh: () => void;
  doc?: Document;
  win?: Window;
};

export function installPullRefresh({ enabled, refresh, doc = document, win = window }: PullOptions): void {
  let x = 0;
  let y = 0;
  const render = (phase: PullPhase, distance: number) => {
    pull.phase = phase;
    pull.distance = distance;
    doc.documentElement.dataset.pullState = phase;
    doc.documentElement.style.setProperty('--pull-distance', `${distance}px`);
  };
  const editing = (target: EventTarget | null) =>
    target instanceof Element && target.closest('input,textarea,select,[contenteditable]') !== null;
  doc.addEventListener(
    'touchstart',
    (event) => {
      if (!enabled() || win.scrollY > 0 || event.touches.length !== 1) return;
      if (doc.querySelector('dialog[open]') || editing(event.target)) return;
      x = event.touches[0].clientX;
      y = event.touches[0].clientY;
      render('tracking', 0);
    },
    { passive: true },
  );
  doc.addEventListener(
    'touchmove',
    (event) => {
      if (!TRACKING.includes(pull.phase)) return;
      if (event.touches.length !== 1 || win.scrollY > 0) {
        render('idle', 0);
        return;
      }
      const next = pullState(pull.phase, event.touches[0].clientX - x, event.touches[0].clientY - y);
      if ((next.phase === 'pulling' || next.phase === 'armed') && event.cancelable) event.preventDefault();
      render(next.phase as PullPhase, next.distance);
    },
    { passive: false },
  );
  doc.addEventListener(
    'touchend',
    () => {
      if (pull.phase === 'armed') {
        render('refreshing', 48);
        refresh();
      } else render('idle', 0);
    },
    { passive: true },
  );
  doc.addEventListener('touchcancel', () => render('idle', 0), { passive: true });
}
