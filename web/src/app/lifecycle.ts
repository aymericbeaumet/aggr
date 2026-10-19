import { untrack } from 'svelte';
import type { ClientPage } from '@aggr-model/ClientPage';
import { installFeedChrome, settlePictures } from './platform';
import { installReadingHeader } from '../article/readingHeader';
import { installSearchIntent } from '../shortcuts/search';
import { pageArrived } from './reader';
import { pageScope } from '../navigation/scope';

/** A page owns its listeners and ends them when Svelte disposes it. */
export function lifecycle(page: ClientPage) {
  return () => untrack(() => {
    document.body.dataset.kind = page.kind;
    document.documentElement.lang = page.site.language;
    const signal = pageScope.renew();
    settlePictures();
    installFeedChrome(signal);
    installReadingHeader(signal);
    installSearchIntent(signal);
    pageArrived();
    return () => pageScope.abort();
  });
}
