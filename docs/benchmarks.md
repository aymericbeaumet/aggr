# Archive size and deployment measurements

Git makes the archive inspectable and portable; it does not make image storage free. This
snapshot of the public demo shows the cost of preserving full image masters and responsive
copies. The current [build budget](build-budget.md) limits the published site while preserving
article text and the Git archive. Measure repository storage separately from deployment size.

## Public demo: September 20, 2026

These are read-only observations of
[`aggr-instance`](https://github.com/aymericbeaumet/aggr-instance), not a controlled benchmark.
The repository was created September 2, 2026, and imports older articles. Its age is **not**
an observation period from which to extrapolate monthly growth.
These runs predate the build budget and age-based media compression; they are a baseline,
not measurements of those features.

- Configuration: [`e62fd64`](https://github.com/aymericbeaumet/aggr-instance/blob/e62fd64ad93cdcb2e0422d9e79d9b512a3ea25b9/aggr.toml),
  50 configured sources, image and preview preservation enabled, unlimited store retention.
- Archive: [`22ab9b0`](https://github.com/aymericbeaumet/aggr-instance/tree/22ab9b04e706ff09a11d5b9b4405bae940420d26),
  2,314 item Markdown files and 20,932 files in total. These file counts differ from the 2,057
  items reported by the latest build; archive files and rendered items are separate measurements.
- Runtime: aggr 1.9.0, Linux amd64, GitHub-hosted `ubuntu-24.04` runner image
  `20260907.300`. Every sampled run restored derived caches successfully; cache restoration does
  not mean every entry was a hit. No CPU model or memory measurement was collected.

| Storage measurement | Bytes | Scope |
| --- | ---: | --- |
| Current archive files | 4,375,241,903 | Sum of blob sizes at the pinned data commit; 4.07 GiB |
| Image files | 4,324,644,167 | JPEG, PNG, GIF and WebP; 98.84% of archive file bytes |
| All other files | 50,597,736 | Markdown, HTML, state and metadata; 48.25 MiB |
| GitHub repository `size` | 3,920,596 KiB | API-reported repository size at observation time; approximately 3.74 GiB |
| Latest Pages artifact | 4,115,522,205 | Uploaded artifact size, not an unpacked site measurement |

The GitHub repository size is not a measured fresh-clone transfer, a count of unique historical
objects, or the current archive file total. A tree sum counts each file path even if multiple
paths reference the same Git blob. The non-image subtotal is **not** a benchmark of an otherwise
identical archive with images disabled: that comparison has not been run.

### Six consecutive scheduled runs

All six scheduled runs on September 20 completed successfully. The build step includes archive
checkout, synchronization, Git publication, and rendering. The internal render timing excludes
the preceding sync. Job time includes setup, cache transfer, artifact upload and deployment;
it excludes the queue before the runner starts. These are incremental runs, not initial imports
or verified no-op runs.

| Run, UTC | Items reported by build | Render | Build step, including sync | Whole job |
| --- | ---: | ---: | ---: | ---: |
| [00:04](https://github.com/aymericbeaumet/aggr-instance/actions/runs/35477775329) | 2,033 | 39.2 s | 194 s | 353 s |
| [04:46](https://github.com/aymericbeaumet/aggr-instance/actions/runs/35489974088) | 2,038 | 35.8 s | 209 s | 360 s |
| [09:43](https://github.com/aymericbeaumet/aggr-instance/actions/runs/35503083105) | 2,043 | 38.9 s | 199 s | 349 s |
| [14:06](https://github.com/aymericbeaumet/aggr-instance/actions/runs/35515468767) | 2,049 | 49.9 s | 284 s | 464 s |
| [17:26](https://github.com/aymericbeaumet/aggr-instance/actions/runs/35525851506) | 2,052 | 50.7 s | 211 s | 384 s |
| [19:41](https://github.com/aymericbeaumet/aggr-instance/actions/runs/35533119708) | 2,057 | 42.3 s | 261 s | 425 s |
| Median | — | 40.75 s | 210 s | 372 s |

In the last run, media and item metadata consumed 27.9 of the 42.3 render seconds. Uploading
the Pages artifact took approximately 145 seconds. The same run recorded two source failures;
successful publication does not imply every source fetched successfully. Standalone sync time,
initial setup time, fresh-clone time, packed local history size and unpacked output size remain
unmeasured.

### Hosting consequence

Every sampled deployment logged a warning that its artifact exceeded the allowed 1 GB.
GitHub documents a **1 GB published-site limit**. Successful deployment above that threshold is
not evidence of a supported hosting budget. The demo needs a rebuild with the media budget or a
host that supports its measured size. The new policy keeps every retained article's text while
reducing the published media. See
[GitHub Pages limits](https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits).

The build now enforces a default 1,000,000,000-byte logical output budget. The reusable workflow
also checks output bytes before uploading to Pages, protecting against older binaries or a
configured budget above the host limit. Decimal GB is conservative relative to GiB; neither
check predicts archive overhead or other platform quotas. `pages: false` keeps the site-artifact
path available for other hosting. See [the build-budget contract](build-budget.md).

## Budget and cache check on a local archive

A September 20, 2026 development build was exercised on an existing local snapshot,
[`76a5f2b`](https://github.com/aymericbeaumet/aggr-instance/tree/76a5f2ba581940085d55209be31590aba2559d6c)
from September 8. This is a different archive from the live-instance table above: 1,060 stored
article files, with 759,008,993 bytes under `items/`, including 743,701,651 image bytes.
The normal duplicate-URL policy produced 803 distinct articles and 257 redirect pages. Every
stored article was accounted for; neither hidden items nor Shorts explain the difference.

The machine was an Apple M4 Pro with 48 GiB RAM, macOS arm64, using the optimized development
profile and release-mode HTML. Other development checks could run concurrently, so timings are
observations, not controlled release-performance claims. The frozen executable's SHA-256 was
`64f4340418ce5b367357525ab95303fb7f12b34b7c60bbedccfdba3301f29c75`.

An isolated local clone shared existing Git objects, had no remote, and used `--data-ref` to avoid
sync, fetching and pushing. Only its config, cache and output were changed. The original
repository's refs and both main/data working-tree statuses were identical before and after.

| Run | Budget | Cache state | Wall time | Result |
| --- | ---: | --- | ---: | --- |
| Initial | 100,000,000 bytes | Cold derived caches | 207.83 s | Refused: the media-free render already needed 150,696,839 bytes |
| Forced render 1 | 200,000,000 bytes | Warm derived caches | 45.95 s | 193,955,811 published bytes; all 803 articles |
| Forced render 2 | 200,000,000 bytes | Warm derived caches | 43.59 s | 193,955,811 published bytes; all 803 articles |

The failed run also rendered 803 articles. Raising the cap allowed publication without dropping
article text. Both successful runs needed two render passes to reserve enough room for text and
required assets. The full-site render cache was removed before each warm run while image
validation, compression and search caches remained. These were actual rerenders, not full-site
cache hits. The differing budgets and cache state prevent treating the cold/warm wall-time ratio
as a measured speedup for an identical workload.

Media preparation took 163.9 seconds on the initial cold pass and 4.1–4.5 seconds per warm pass.
The compression cache contained 1,017 files totaling 57,275,712 bytes. Their content digest was
identical before and after both warm runs; file identities, sizes and modification times also
stayed unchanged, with every modification time predating the first warm run. No compressed cache
file was rewritten. Separate unit tests verify that hits bypass the compressor itself.

The measurements exposed a 40-byte render-cache marker added after the original size check. The
budget now accounts for that marker before enforcement and tests the boundary through cache
storage and restoration. The successful measurements above include all final published files and
were already below their 200 MB limit.

### What occupies the non-media output?

The 200 MB deployment contained 42,026,466 bytes of article images/previews and 151,929,345 other
bytes. Its main non-media file categories were:

| Files | Bytes |
| --- | ---: |
| XML, mostly full-content feeds | 47,852,661 |
| JSON, including feeds and article exports | 44,304,133 |
| HTML | 34,978,659 |
| Markdown exports | 6,387,159 |
| reStructuredText exports | 4,297,803 |
| Plain-text outputs | 4,253,040 |
| JavaScript | 1,030,740 |
| CSS | 126,141 |

The largest files were Qwen's Atom/RSS/JSON feeds at approximately 5.99/5.99/4.87 MB, followed
by overlapping tag feeds of approximately 2.3–3.1 MB. Feed and portable-representation repetition
therefore matters more to this archive's remaining size than CSS or JavaScript. These figures are
a baseline for further output changes, not evidence that every source mix has the same balance.

## Reproduce the GitHub observations

With GitHub CLI authentication, these commands fetch metadata and logs without downloading the
multi-gigabyte archive. Pin the data SHA before reading the tree. Refuse a truncated API response;
larger archives need subtree traversal or a local checkout.

```sh
gh api repos/aymericbeaumet/aggr-instance --jq '{created_at,size}'
gh api 'repos/aymericbeaumet/aggr-instance/git/trees/22ab9b04e706ff09a11d5b9b4405bae940420d26?recursive=1' > tree.json
python3 - <<'PY'
import json
from pathlib import Path

tree = json.loads(Path("tree.json").read_text())
if tree["truncated"]:
    raise SystemExit("Tree is truncated; totals would be incomplete")
files = [entry for entry in tree["tree"] if entry["type"] == "blob"]
images = {".jpg", ".jpeg", ".png", ".gif", ".webp"}
print("files:", len(files))
print("bytes:", sum(entry["size"] for entry in files))
print("image bytes:", sum(entry["size"] for entry in files
                          if Path(entry["path"]).suffix.lower() in images))
print("item Markdown files:", sum(entry["path"].startswith("items/")
                                  and entry["path"].endswith(".md") for entry in files))
PY
gh run view 35533119708 --repo aymericbeaumet/aggr-instance --json jobs
gh run view 35533119708 --repo aymericbeaumet/aggr-instance --log > run.log
gh api repos/aymericbeaumet/aggr-instance/actions/runs/35533119708/artifacts \
  --jq '.artifacts[] | {name,size_in_bytes,expired}'
```

Logs and artifacts have retention periods. The fixed references and table above preserve the
observation after those run artifacts expire; the repository-size query always reports current
metadata, not a historical value.

## Plan storage and hosting separately

Start with a few sources and measure an initial import, then ordinary daily growth separately.
The defaults keep retained article text in the site, prefer recent media at full quality, and
compress media belonging to older articles. If necessary, the build omits lower-priority local
media to fit the publication budget. The masters remain in Git. Adjust the output policy with:

```toml
[site]
build_max_bytes = 1000000000
media_full_quality_days = 30
```

Omitted media falls back to publisher URLs and is not guaranteed offline. Compression and
publication omission do not reduce Git storage; both settings above shape the published site, not
the archive behind it.

Git storage is decided by `[fetch] images`, because images are nearly all of an archive's bytes.
In the archive measured above, lossless WebP renditions alone outweighed every master combined.

```toml
[fetch]
images = { mode = "compact", quality = 72, max_axis = 1600 }
```

`"compact"` archives one bounded copy per image and no renditions, which removes both the
rendition bytes and most of each master. Measured on the twelve largest masters in this archive,
the six stills went from 90,710,039 bytes with their renditions to 7,555,854 (-91.7%). The six
animated GIFs did not move: all were already inside the 1600-pixel bound, and re-encoding a
frame-differenced animation is larger, so each kept its exact bytes. Animations are therefore the
part of an archive `"compact"` does least for. `"remote"` stores no images at all. Neither applies
retroactively: they change what later runs archive, and previously stored images are untouched.
Compaction also discards the publisher's exact bytes for good, so choose it for sources you want
to read rather than preserve. See [source preservation](sources.md#article-images).

Record current-tree bytes, Git object storage, generated-site bytes and deployment duration as
separate series, with source count, imported item count, version and cache conditions. Do not
estimate monthly growth from initial backfill. Include image-heavy sources in capacity planning.

`[site] max_items` and `max_age_days` limit the home feed, not the complete published archive.
Leave `[store]` retention unset to preserve all captured articles in the current archive. It is
independent of the build budget: deliberately enabling retention removes articles from the current
tree, while normal deletion commits cannot reclaim append-only Git history. Avoid rewriting that
history to meet a storage budget: existing blob permalinks depend on it. For larger archives,
choose appropriate repository and static hosting capacity; see [hosting](hosting.md) and
[the Git model](git-model.md).
