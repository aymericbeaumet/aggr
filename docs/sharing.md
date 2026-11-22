# Sharing a reader

A reading list, filtered view or archived article can be useful on its own. Share the part another
person can use, with a working original link and enough context to understand what was captured.

## Introduce the project

A short description you can adapt when sharing aggr:

> aggr is a feed reader that saves captured articles as Markdown in a Git branch and publishes a
> searchable static site. A scheduled job keeps it up to date; the reader needs no database or
> always-running application server. You can fork the example instance or run the prebuilt CLI.

Pair the [repository](https://github.com/aymericbeaumet/aggr) with the
[live reader](https://aggr.aymericbeaumet.com), so people can try it before setting anything up.
The README includes desktop and phone screenshots, local preview commands, and
[setup instructions](../readme.md#create-your-reader).

For a Hacker News or Reddit introduction, lead with the concrete workflow or problem that made
you build your reader. Show one useful source collection or an article's Markdown history. Explain
the tradeoffs below and invite specific feedback about discovery, reading, or setup. Keep claims
about speed, storage, and cost tied to measured inputs; a small example is not a benchmark for an
unbounded archive.

## Links and portable exports

- Link directly to an article. Its metadata keeps the publisher and original URL visible.
- Select a passage and choose **Share** to send a link that selects those words when opened in
  the reader. See [sharing a passage](reading.md#sharing-a-passage).
- Choose a source, category or search query and copy the address. Query URLs preserve the filter.
- Use **Download subscriptions (OPML)** on Browse to import the sources into another feed reader.
- Use **Subscribe to this reader** for its combined RSS feed, or **View reading list** for its
  configuration. Keep a small source collection when it makes the list easier to reuse.
- Link to an article's stored Markdown when explaining how the Git archive works.

Reading state and preferences belong to each browser. Sharing a URL does not share those settings
or synchronize another person's reading progress. Check attribution and permissions before
republishing captured content; see [hosting](hosting.md#publication-and-search-indexing).

## Make an instance easy to reuse

Give the reader a clear topic and a small starting collection. Link its source configuration and
explain which parts a new owner should replace. A focused reading list gives someone a useful
starting point before they learn the configuration format.

Before sharing the URL, open it in a fresh browser session and check that an article, a source
page, search, and the OPML download work. Test the repository-subdirectory URL if that is how the
reader is hosted. Keep the original publisher links visible and use a public URL for anything
the recipient is expected to open.

## Set accurate expectations

The [starter](../examples/starter.toml) is a small first configuration. The maintainer's full
instance is a separate example: replace its configuration before enabling Actions, as the
[README](../readme.md#create-your-reader) explains. Confirm that the binary and reusable workflow
versions support the options in use. Measure complete instance job time separately from local
rendering time.

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
