import type { ClientPage } from '@aggr-model/ClientPage';

/** The page on screen: its typed model, opaque prose and address. */
export type Shown = {
  model: ClientPage;
  bodyHtml: string;
  href: string;
};

class PageState {
  current = $state.raw<Shown | null>(null);
  /** Absolute URL of the site root, fixed for the visit; `''` until the app boots. */
  root = '';
  get model(): ClientPage | null {
    return this.current?.model ?? null;
  }

  get bodyHtml(): string {
    return this.current?.bodyHtml ?? '';
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
