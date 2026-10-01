import type { ClientPage } from '../generated/ClientPage';

/** The page on screen: its model, the region it adopted and the address it is shown at. */
export type Shown = {
  model: ClientPage;
  content: Element | null;
  href: string;
};

class PageState {
  current = $state.raw<Shown | null>(null);
  /** Absolute URL of the site root, fixed for the visit; `''` until the app boots. */
  root = '';
  /** The site icon as the static head resolved it, for the brand link; `null` when unknown. */
  icon: string | null = null;

  get model(): ClientPage | null {
    return this.current?.model ?? null;
  }

  get content(): Element | null {
    return this.current?.content ?? null;
  }

  /** The address the page is shown at, with its query; null on the server. */
  get href(): string | null {
    return this.current?.href ?? null;
  }

  get kind(): string {
    return this.current?.model.kind ?? '';
  }

  show(shown: Shown): void {
    this.current = shown;
  }
}

export const page = new PageState();
