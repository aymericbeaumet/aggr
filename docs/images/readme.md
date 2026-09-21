# README captures

The three `*-placeholder.svg` files are explicit placeholders, not product screenshots.
Replace the corresponding image links in the root README with these captures:

| File | Capture |
|---|---|
| `reader.webp` | Desktop and mobile side by side, with readable titles and visible source names. Use the same theme and articles in both. |
| `updates.gif` | About 15 seconds showing a completed deployment delivering a new item to an already-open feed without a page refresh. Keep enough surrounding UI to make the change visible. |
| `archive.webp` | The instance's `aggr` branch showing article Markdown files and, if legible, an opened file with its metadata and body. |

Use your own or permissioned article content for featured captures. Remove unrelated browser
chrome and personal information. Keep images reasonably small so the README loads quickly on
mobile, and update alt text to describe the real capture. Delete placeholders after replacing them.

The update recording demonstrates the browser noticing a completed deployment. Do not imply the
publisher-to-reader pipeline runs in 15 seconds; fetching and deployment happen on a separate schedule.
