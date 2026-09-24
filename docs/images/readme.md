# README captures

`reader-desktop.png` and `reader-mobile.png` are the reader as it ships, captured from a two-source
instance created by `aggr init` and served by `aggr dev`, at a device scale factor of 1.

To retake them, run `aggr dev` against an instance whose articles you may publish, then screenshot
`http://127.0.0.1:7319/` at 1180×760 and 390×760 with the same theme in both. Keep unrelated
browser chrome and personal information out of the frame, keep the files small enough that the
README loads quickly on a phone, and describe the real capture in the alt text.

A recording of a deployment reaching an already-open feed would suit the "Why aggr?" section, as
about fifteen seconds showing a new item arriving without a page refresh. It must not imply that
the publisher-to-reader pipeline runs in fifteen seconds: fetching and deployment happen on their
own schedule, and only the browser's check for a completed deployment is that quick.
