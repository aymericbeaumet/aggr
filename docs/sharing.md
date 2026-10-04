# Sharing a reader

A reading list, filtered view or archived article can be useful on its own. Share the part another
person can use, with a working original link and enough context to understand what was captured.

## Links and portable exports

- Link directly to an article. Its metadata keeps the publisher and original URL visible.
- Choose a source, category or search query and copy the address. Query URLs preserve the filter.
- Use **Download subscriptions (OPML)** on Browse to import the sources into another feed reader.
- Use **Subscribe to this reader** for its combined RSS feed, or **View reading list** for its
  configuration. Keep a small source collection when it makes the list easier to reuse.
- Link to an article's stored Markdown when explaining how the Git archive works.

Reading state and preferences belong to each browser. Sharing a URL does not share those settings
or synchronize another person's reading progress. Check attribution and permissions before
republishing captured content; see [hosting](hosting.md#publication-and-search-indexing).

## An optional introduction

A reader intended for visitors can explain itself with a collapsible introduction:

```toml
[site.params]
introduction = true
```

It links to the setup instructions, reading list and subscription download. It uses native HTML,
adds no external service, and stays collapsed during navigation after a visitor closes it.

The [starter](../examples/starter.toml) is a small first configuration. The maintainer's full
instance is a separate example: replace its configuration before enabling Actions, as the
[README](../readme.md#create-your-reader) explains. Confirm that the binary and reusable workflow
versions support the options in use. Measure complete instance job time separately from local
rendering time.

## Set accurate expectations

Defaults reuse feed content and keep media at the publisher. A feed can contain only summaries;
original-page extraction and local media capture are optional. Publisher-hosted media still needs
the publisher and a connection. The append-only Git history and published site consume separate
storage, and static hosts have their own limits.

When comparing build time or storage, use equivalent inputs and state the selected content/media
policies. Keep first imports, cold rendering, warm rendering and deployment timings separate;
see [benchmarks](benchmarks.md) and [performance](performance.md). The controlled
[first-visit budget](client.md#first-visit-budget) includes worker installation. Measure a hosted
reader separately on the relevant devices and connections.

Useful verification includes a fresh setup, working shared query URLs and OPML import, narrow
screens, keyboard use, disabled JavaScript, explicit offline downloads and failed-source recovery.
