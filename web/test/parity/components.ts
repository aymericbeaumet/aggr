import type { Component } from 'svelte';
import App from '../../src/app/App.svelte';
import Header from '../../src/app/Header.svelte';
import ArticleFooter from '../../src/article/ArticleFooter.svelte';
import ArticleHeader from '../../src/article/ArticleHeader.svelte';
import ListHead from '../../src/feed/ListHead.svelte';
import Metadata from '../../src/feed/Metadata.svelte';
import Pager from '../../src/feed/Pager.svelte';
import RelatedCard from '../../src/feed/RelatedCard.svelte';
import Row from '../../src/feed/Row.svelte';
import Toolbar from '../../src/feed/Toolbar.svelte';

// Fixtures carry arbitrary props, so the map is typed loosely on purpose.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type ParityComponent = Component<any>;

/** The components that mirror a minijinja partial, by the name the fixture generator uses. */
export const components: Record<string, ParityComponent> = {
  App,
  ArticleFooter,
  ArticleHeader,
  Header,
  ListHead,
  Metadata,
  Pager,
  RelatedCard,
  Row,
  Toolbar,
};
