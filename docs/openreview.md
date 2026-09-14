# OpenReview papers

Heavy article ingestion recognizes OpenReview forum URLs and same-origin challenge URLs that
refer back to a forum. It uses the official public title-search API, then requires the exact
requested note ID and a public top-level submission. Similar titles and private notes cannot
replace the requested paper. Feed identity, dates, title, and existing author attribution stay intact.

The API supplies the abstract, optional plain-language summary, and an official PDF link.
When the same response also identifies an arXiv copy, aggr follows it only if its title,
paper hash, and complete ordered author list match the requested public submission. Conflicting
arXiv identifiers disable the alternate. The resulting article labels the copy as the authors'
arXiv preprint, which can differ from the conference version.

Usable arXiv HTML passes through normal extraction and sanitization; relative figures and links
resolve against arXiv. MathML formulas with TeX alternative text become one readable code span,
avoiding duplicate hidden annotations; the paper's PDF remains linked for original typesetting.
When HTML is unavailable, a successful PDF GET with PDF content type and
signature can enable the document reader. A challenged or unavailable PDF remains an ordinary
link instead of covering the readable abstract with a broken embed. Neither path solves challenges,
launches a browser, runs Python, or uses an extraction service.

Requests use the shared timeout, byte limit, pacing, and conditional response cache. Source
credentials stay scoped to their declaring origin. API errors can reuse previously verified cached
metadata; otherwise feed content remains available. Light sources do not request this enrichment.
Normal ingestion preserves the original item path. Existing items require explicit refresh; the
CLI currently has no per-item refresh selector, and `--refresh` processes existing feed-present
entries only. Dev refresh changes its isolated cache, not the Git data branch.

OpenReview documents [public note search and PDF routes](https://docs.openreview.net/reference/api-v2/openapi-definition)
and [submission identity and readers](https://docs.openreview.net/getting-started/using-the-api/objects-in-openreview/introduction-to-notes).
