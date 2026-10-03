/**
 * The polite live region at the top of the page (`#reader-announcement`): navigations and new
 * items are read out from here. The region is emptied before it is filled in the next frame,
 * so the same message twice is read twice.
 */
class Announcer {
  message = $state('');
  private frame = 0;

  say(text: string, win: Window | null = typeof window === 'undefined' ? null : window): void {
    this.message = '';
    if (!win) {
      this.message = text;
      return;
    }
    win.cancelAnimationFrame(this.frame);
    this.frame = win.requestAnimationFrame(() => {
      this.message = text;
    });
  }
}

export const announcer = new Announcer();
