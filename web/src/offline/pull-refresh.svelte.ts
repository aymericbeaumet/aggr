/**
 * Pull-to-refresh for the installed app, where the browser's own is out of reach: a single
 * touch dragged down from the top reveals a small indicator; the page stays in place. A
 * deliberate release reloads keeping the reader's place. Motion stays on the indicator's
 * compositor layer, with no inherited per-frame styles on the document.
 */

export type PullPhase = 'idle' | 'tracking' | 'pulling' | 'armed' | 'settling' | 'refreshing';

const TRACKING: readonly PullPhase[] = ['tracking', 'pulling', 'armed'];
const ARM_DISTANCE = 84;
const DISARM_DISTANCE = 72;
const SLOP = 6;
const resistedDistance = (dy: number) => 80 * -Math.expm1(-(dy - SLOP) / 100);

/** Where a touch that started at the top leaves the gesture, from how far it moved. */
export function pullState(phase: PullPhase, dx: number, dy: number): { phase: PullPhase; distance: number } {
  if (!TRACKING.includes(phase)) return { phase, distance: 0 };
  if (dy <= 0 || Math.abs(dx) > dy) return { phase: 'idle', distance: 0 };
  if (dy < SLOP) return { phase: phase === 'tracking' ? 'tracking' : 'pulling', distance: 0 };
  return {
    phase: dy >= (phase === 'armed' ? DISARM_DISTANCE : ARM_DISTANCE) ? 'armed' : 'pulling',
    distance: resistedDistance(dy),
  };
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

export function installPullRefresh({ enabled, refresh, doc = document, win = window }: PullOptions): () => void {
  let x = 0;
  let y = 0;
  let distance = 0;
  let paintFrame: number | null = null;
  let refreshFrame: number | null = null;
  let settlingTimer: number | undefined;
  let listening = false;
  const indicator = doc.getElementById('pull-refresh');
  const symbol = indicator?.querySelector<SVGElement>('.pull-refresh-symbol');
  const ring = indicator?.querySelector<SVGElement>('.pull-refresh-ring');
  const moveOptions = { passive: false };
  const phase = (phase: PullPhase) => {
    if (pull.phase === phase) return;
    pull.phase = phase;
    doc.documentElement.dataset.pullState = phase;
  };
  const paint = (next: number) => {
    distance = next;
    if (paintFrame !== null) return;
    paintFrame = win.requestAnimationFrame(() => {
      paintFrame = null;
      if (!indicator) return;
      indicator.style.transform = `translate3d(-50%, ${distance - 40}px, 0)`;
      indicator.style.opacity = String(Math.min(1, distance / 24));
      const progress = pull.phase === 'armed' || pull.phase === 'refreshing' ? 1 : Math.min(1, distance / resistedDistance(ARM_DISTANCE));
      if (symbol) symbol.style.transform = `rotate(${progress * 180}deg)`;
      if (ring) ring.style.strokeDashoffset = String(100 * (1 - progress));
    });
  };
  const stopMoving = () => {
    if (listening) doc.removeEventListener('touchmove', move);
    listening = false;
  };
  const reset = () => {
    stopMoving();
    if (paintFrame !== null) win.cancelAnimationFrame(paintFrame);
    if (refreshFrame !== null) win.cancelAnimationFrame(refreshFrame);
    win.clearTimeout(settlingTimer);
    paintFrame = refreshFrame = null;
    settlingTimer = undefined;
    distance = 0;
    phase('idle');
    indicator?.style.removeProperty('transform');
    indicator?.style.removeProperty('opacity');
    symbol?.style.removeProperty('transform');
    ring?.style.removeProperty('stroke-dashoffset');
  };
  const settle = () => {
    stopMoving();
    if (!distance || doc.documentElement.dataset.motion === 'off' || win.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      reset();
      return;
    }
    phase('settling');
    paint(0);
    // transitionend is not guaranteed when the app is backgrounded or styles change.
    settlingTimer = win.setTimeout(reset, 280);
  };
  const selected = () => doc.getSelection()?.isCollapsed === false;
  const ownsGesture = (target: EventTarget | null) => {
    if (!(target instanceof Element)) return false;
    if (target.closest('a,button,input,textarea,select,label,summary,audio,video,iframe,object,embed,canvas,pre,table,[contenteditable],[role="slider"],[data-no-pull-refresh],[data-site-navigation]')) return true;
    // A nested scroller owns the whole gesture, including overscroll at either end.
    for (let node: Element | null = target; node && node !== doc.body && node !== doc.documentElement; node = node.parentElement) {
      const style = win.getComputedStyle(node);
      if ((node.scrollHeight > node.clientHeight + 1 && /^(auto|scroll)$/.test(style.overflowY)) ||
          (node.scrollWidth > node.clientWidth + 1 && /^(auto|scroll)$/.test(style.overflowX))) return true;
    }
    return false;
  };
  const start = (event: TouchEvent) => {
    if (pull.phase === 'refreshing') return;
    if (event.touches.length !== 1) {
      cancel();
      return;
    }
    reset();
    if (!enabled() || win.scrollY > 0) return;
    if (doc.querySelector('dialog[open]') || selected() || ownsGesture(event.target) || (win.visualViewport?.scale ?? 1) > 1) return;
    x = event.touches[0].clientX;
    y = event.touches[0].clientY;
    if (x < 24 || x > win.innerWidth - 24) return;
    phase('tracking');
    doc.addEventListener('touchmove', move, moveOptions);
    listening = true;
  };
  const move = (event: TouchEvent) => {
    if (!TRACKING.includes(pull.phase)) return;
    if (event.touches.length !== 1 || win.scrollY > 0 || selected() || (win.visualViewport?.scale ?? 1) > 1) {
      settle();
      return;
    }
    const next = pullState(pull.phase, event.touches[0].clientX - x, event.touches[0].clientY - y);
    if ((next.phase === 'pulling' || next.phase === 'armed') && event.cancelable) event.preventDefault();
    if (next.phase === 'idle') settle();
    else {
      phase(next.phase);
      if (next.phase !== 'tracking') paint(next.distance);
    }
  };
  const end = (event: TouchEvent) => {
    if (!TRACKING.includes(pull.phase)) return;
    stopMoving();
    const released = event.changedTouches?.[0];
    if (released && pull.phase !== 'tracking') {
      const next = pullState(pull.phase, released.clientX - x, released.clientY - y);
      if (next.phase === 'idle') {
        settle();
        return;
      }
      phase(next.phase);
    }
    if (pull.phase === 'armed' && !event.touches.length && !selected() && win.scrollY <= 0 && (win.visualViewport?.scale ?? 1) <= 1) {
      phase('refreshing');
      paint(44);
      // Let the indicator paint before reload begins, without delaying the request for a tween.
      refreshFrame = win.requestAnimationFrame(() => {
        refreshFrame = win.requestAnimationFrame(() => {
          refreshFrame = null;
          refresh();
        });
      });
    } else settle();
  };
  const cancel = () => { if (TRACKING.includes(pull.phase)) settle(); };
  const settled = (event: TransitionEvent) => {
    if (pull.phase === 'settling' && event.target === indicator && event.propertyName === 'transform') reset();
  };
  const dispose = () => {
    reset();
    doc.removeEventListener('touchstart', start);
    doc.removeEventListener('touchend', end);
    doc.removeEventListener('touchcancel', cancel);
    indicator?.removeEventListener('transitionend', settled);
    win.removeEventListener('pagehide', reset);
  };
  doc.addEventListener('touchstart', start, { passive: true });
  doc.addEventListener('touchend', end, { passive: true });
  doc.addEventListener('touchcancel', cancel, { passive: true });
  indicator?.addEventListener('transitionend', settled);
  win.addEventListener('pagehide', reset);
  return dispose;
}
