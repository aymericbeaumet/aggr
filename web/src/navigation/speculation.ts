/** A reader who asked the browser to save data, or who is on a slow link, gets no guesses. */
export function frugal(connection?: { saveData?: boolean; effectiveType?: string } | null): boolean {
  return Boolean(connection?.saveData) || /2g$/.test(connection?.effectiveType || '');
}
