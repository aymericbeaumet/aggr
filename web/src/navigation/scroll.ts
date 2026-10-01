/**
 * Where each history entry was scrolled to, by the key its state carries, and where each list
 * was last left, by address. Like a native tab, a list reached again through its tab picks up
 * where the reader was; asking for the list already on screen goes to its top.
 */
export class ScrollMemory {
  private readonly scrolls = new Map<string, number>();
  private readonly places = new Map<string, number>();

  constructor(readonly placeLimit = 50) {}

  remember(entry: string, y: number): void {
    this.scrolls.set(entry, y);
  }

  recall(entry: string): number | undefined {
    return this.scrolls.get(entry);
  }

  /** A list is being left at `y`; the oldest list is forgotten past the limit. */
  leaveList(address: string, y: number): void {
    this.places.delete(address);
    this.places.set(address, y);
    while (this.places.size > this.placeLimit) {
      const first = this.places.keys().next();
      if (first.done) break;
      this.places.delete(first.value);
    }
  }

  place(address: string): number | undefined {
    return this.places.get(address);
  }
}

/** A fresh key for a history entry. */
export function newKey(): string {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
}
