interface TouchTabPorts<T> {
  activate(target: T): void;
  press(target: T | null): void;
  now(): number;
}

type TouchTabState<T> = { phase: 'idle' } | { phase: 'pressed'; id: number; target: T; x: number; y: number };

export function createTouchTabActivation<T>(ports: TouchTabPorts<T>) {
  let state: TouchTabState<T> = { phase: 'idle' };
  let consumed: { target: T; time: number } | null = null;
  function cancel() {
    state = { phase: 'idle' };
    ports.press(null);
  }
  function moved(x: number, y: number) {
    return state.phase === 'pressed' && Math.hypot(x - state.x, y - state.y) > 10;
  }
  return {
    start(id: number, target: T, x: number, y: number, primary: boolean) {
      consumed = null;
      if (!primary || state.phase === 'pressed') { cancel(); return; }
      state = { phase: 'pressed', id, target, x, y };
      ports.press(target);
    },
    move(id: number, x: number, y: number) {
      if (state.phase === 'pressed' && state.id === id && moved(x, y)) cancel();
    },
    end(id: number, target: T | null, x: number, y: number) {
      if (state.phase !== 'pressed' || state.id !== id) return false;
      const pressed = state.target;
      const activate = target === pressed && !moved(x, y);
      cancel();
      if (!activate) return false;
      consumed = { target: pressed, time: ports.now() };
      ports.activate(pressed);
      return true;
    },
    consumeClick(target: T, detail: number) {
      return detail > 0 && consumed?.target === target && ports.now() - consumed.time < 750;
    },
    cancel,
    reset() { cancel(); consumed = null; },
  };
}

/** Touch tabs activate on release; the following compatibility click must not navigate twice. */
export function mountTouchTabs(bar: HTMLElement) {
  const document = bar.ownerDocument;
  let pressed: HTMLAnchorElement | null = null;
  const taps = createTouchTabActivation<HTMLAnchorElement>({
    activate: link => link.click(),
    now: () => performance.now(),
    press(link) {
      pressed?.removeAttribute('data-pressed');
      pressed = link;
      pressed?.setAttribute('data-pressed', '');
    },
  });
  const linkAt = (target: EventTarget | null) => {
    const link = target instanceof Element ? target.closest<HTMLAnchorElement>('a[data-route]') : null;
    return link && bar.contains(link) ? link : null;
  };
  const down = (event: PointerEvent) => {
    if (event.pointerType !== 'touch') { taps.reset(); return; }
    const link = linkAt(event.target);
    if (link) taps.start(event.pointerId, link, event.clientX, event.clientY, event.isPrimary);
    else taps.cancel();
  };
  const anotherTouch = (event: PointerEvent) => {
    if (event.pointerType === 'touch' && !event.isPrimary) taps.cancel();
  };
  const move = (event: PointerEvent) => taps.move(event.pointerId, event.clientX, event.clientY);
  const up = (event: PointerEvent) => {
    if (event.pointerType !== 'touch') return;
    const link = linkAt(document.elementFromPoint(event.clientX, event.clientY));
    if (taps.end(event.pointerId, link, event.clientX, event.clientY)) event.preventDefault();
  };
  const click = (event: MouseEvent) => {
    const link = linkAt(event.target);
    if (link && taps.consumeClick(link, event.detail)) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
  };
  const context = (event: MouseEvent) => {
    if ('pointerType' in event && event.pointerType === 'touch') {
      taps.cancel();
      event.preventDefault();
    }
  };
  document.addEventListener('pointerdown', anotherTouch, true);
  bar.addEventListener('pointerdown', down);
  bar.addEventListener('pointermove', move);
  bar.addEventListener('pointerup', up);
  bar.addEventListener('pointercancel', taps.cancel);
  bar.addEventListener('click', click, true);
  bar.addEventListener('contextmenu', context);
  return () => {
    taps.reset();
    document.removeEventListener('pointerdown', anotherTouch, true);
    bar.removeEventListener('pointerdown', down);
    bar.removeEventListener('pointermove', move);
    bar.removeEventListener('pointerup', up);
    bar.removeEventListener('pointercancel', taps.cancel);
    bar.removeEventListener('click', click, true);
    bar.removeEventListener('contextmenu', context);
  };
}
