export interface ArticleSwipePorts {
  previous(): string | null;
  next(): string | null;
  navigate(url: string): void;
  isBusy(): boolean;
}

type Direction = "previous" | "next";
type Origin = {
  pointer: number;
  x: number;
  y: number;
  time: number;
  scrollX: number;
  scrollY: number;
  previous: string | null;
  next: string | null;
};
type Gesture =
  | { phase: "idle" }
  | { phase: "cancelled" }
  | { phase: "tracking"; origin: Origin }
  | { phase: "locked"; origin: Origin; direction: Direction };

const EXCLUDED = "a[href],button,input,textarea,select,summary,label,[contenteditable]:not([contenteditable=false]),[role=button],[role=slider],[role=tab],[data-no-swipe],audio,video,iframe,object,embed,canvas,pre,.table-scroll";
const EDGE = 24;
const SLOP = 12;
const TRAVEL = 64;
const MAX_DURATION = 1200;

function advance(gesture: Gesture, x: number, y: number): Gesture {
  if (gesture.phase === "idle" || gesture.phase === "cancelled") return gesture;
  const dx = x - gesture.origin.x;
  const dy = y - gesture.origin.y;
  if (Math.max(Math.abs(dx), Math.abs(dy)) < SLOP) return gesture;
  if (Math.abs(dy) > 48 || Math.abs(dx) < Math.abs(dy) * 1.5) return { phase: "cancelled" };
  const direction = dx < 0 ? "next" : "previous";
  if (!gesture.origin[direction] || (gesture.phase === "locked" && gesture.direction !== direction)) return { phase: "cancelled" };
  return { phase: "locked", origin: gesture.origin, direction };
}

/** Swipe only starts on reading surfaces; links, native controls and horizontal scrollers keep their gestures. */
export function mountArticleSwipe(root: HTMLElement, ports: ArticleSwipePorts): () => void {
  const document = root.ownerDocument;
  const view = document.defaultView;
  if (!view) return () => {};
  let gesture: Gesture = { phase: "idle" };
  const pointers = new Set<number>();
  let disposed = false;
  let clickGuard: (() => void) | undefined;

  function selected() {
    return view!.getSelection()?.isCollapsed === false;
  }

  function eligible(target: EventTarget | null): boolean {
    if (!target || (target as Node).nodeType !== 1 || !root.contains(target as Node)) return false;
    let element = target as HTMLElement;
    if (element.closest(EXCLUDED)) return false;
    while (element !== root) {
      if (element.scrollWidth > element.clientWidth + 1 && /auto|scroll/.test(view!.getComputedStyle(element).overflowX)) return false;
      if (!element.parentElement) return false;
      element = element.parentElement;
    }
    return true;
  }

  function suppressClick(release: PointerEvent) {
    clickGuard?.();
    // Compatibility clicks can arrive after Swup has disposed this page. Keep this
    // narrowly matched guard until that click, the next pointer, or its short expiry.
    const cleanup = () => {
      clearTimeout(timer);
      document.removeEventListener("click", click, true);
      document.removeEventListener("pointerdown", cleanup, true);
      if (clickGuard === cleanup) clickGuard = undefined;
    };
    const click = (event: MouseEvent) => {
      if (event.detail === 0 || Math.abs(event.clientX - release.clientX) > 32 || Math.abs(event.clientY - release.clientY) > 32) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      cleanup();
    };
    const timer = setTimeout(cleanup, 500);
    clickGuard = cleanup;
    document.addEventListener("click", click, true);
    document.addEventListener("pointerdown", cleanup, true);
  }

  function down(event: PointerEvent) {
    if (event.pointerType !== "touch" || disposed) return;
    clickGuard?.();
    pointers.add(event.pointerId);
    if (pointers.size !== 1 || !event.isPrimary) {
      gesture = { phase: "cancelled" };
      return;
    }
    if (ports.isBusy() || selected() || !eligible(event.target) || event.clientX <= EDGE || event.clientX >= view!.innerWidth - EDGE) return;
    gesture = {
      phase: "tracking",
      origin: {
        pointer: event.pointerId, x: event.clientX, y: event.clientY, time: event.timeStamp,
        scrollX: view!.scrollX, scrollY: view!.scrollY,
        previous: ports.previous(), next: ports.next(),
      },
    };
  }

  function update(event: PointerEvent) {
    if (event.pointerType !== "touch" || gesture.phase === "idle" || gesture.phase === "cancelled") return;
    if (event.pointerId !== gesture.origin.pointer) return;
    const origin = gesture.origin;
    if (ports.isBusy() || selected() || event.timeStamp - origin.time > MAX_DURATION || Math.abs(view!.scrollX - origin.scrollX) > 2 || Math.abs(view!.scrollY - origin.scrollY) > 2) {
      gesture = { phase: "cancelled" };
      return;
    }
    gesture = advance(gesture, event.clientX, event.clientY);
    if (gesture.phase === "locked" && event.cancelable) event.preventDefault();
  }

  function up(event: PointerEvent) {
    if (event.pointerType !== "touch") return;
    update(event);
    let destination: string | null = null;
    if (gesture.phase === "locked" && gesture.origin.pointer === event.pointerId) {
      const dx = Math.abs(event.clientX - gesture.origin.x);
      const dy = Math.abs(event.clientY - gesture.origin.y);
      if (dx >= TRAVEL && dx >= dy * 2) destination = gesture.origin[gesture.direction];
      suppressClick(event);
    }
    pointers.delete(event.pointerId);
    gesture = { phase: pointers.size ? "cancelled" : "idle" };
    if (destination) ports.navigate(destination);
  }

  function cancel(event: PointerEvent) {
    if (event.pointerType !== "touch") return;
    pointers.delete(event.pointerId);
    gesture = { phase: pointers.size ? "cancelled" : "idle" };
  }

  document.addEventListener("pointerdown", down, { passive: true, capture: true });
  document.addEventListener("pointermove", update, { passive: false, capture: true });
  document.addEventListener("pointerup", up, { passive: false, capture: true });
  document.addEventListener("pointercancel", cancel, { passive: true, capture: true });
  return () => {
    if (disposed) return;
    disposed = true;
    gesture = { phase: "idle" };
    pointers.clear();
    document.removeEventListener("pointerdown", down, true);
    document.removeEventListener("pointermove", update, true);
    document.removeEventListener("pointerup", up, true);
    document.removeEventListener("pointercancel", cancel, true);
  };
}
