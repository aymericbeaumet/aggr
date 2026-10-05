/** A reader who asked the browser to save data, or who is on a slow link, gets no guesses. */
export function frugal(connection?: { saveData?: boolean; effectiveType?: string } | null): boolean {
  return Boolean(connection?.saveData) || /2g$/.test(connection?.effectiveType || '');
}

/** Pages one screen may fetch before anyone asks for them. Intent is never counted. */
export class Speculation {
  count = 0;

  constructor(readonly limit = 16) {}

  /** A new screen starts a fresh allowance. */
  reset(): void {
    this.count = 0;
  }

  /** Whether a guess may be fetched; a page already known costs nothing. */
  admit(known: boolean): boolean {
    if (known) return true;
    if (this.count >= this.limit) return false;
    this.count += 1;
    return true;
  }
}
