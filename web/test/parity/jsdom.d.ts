// The parity normaliser needs only jsdom's constructor and window; `@types/jsdom` is not a
// dependency, so this declares that much.
declare module 'jsdom' {
  export class JSDOM {
    constructor(html?: string, options?: Record<string, unknown>);
    readonly window: Window & typeof globalThis;
  }
}
