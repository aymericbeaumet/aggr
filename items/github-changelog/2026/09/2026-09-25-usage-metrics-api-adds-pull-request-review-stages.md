---
title: Usage metrics API adds pull request review stages
link: https://github.blog/changelog/2026-09-25-usage-metrics-api-adds-pull-request-review-stages
source: github-changelog
published: 2026-09-25T21:09:40Z
updated: 2026-09-25T21:09:40Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- Allison
labels:
- account management
- copilot
- enterprise management tools
- improvement
summary: The enterprise and organization repository-level Copilot usage metrics reports now break down how long pull requests spend in each stage of review. A new pull_request_review_times array on each repos-1-day row… The post Usage metrics API adds pull request review stages appeared first on The GitHub Blog.
content: extracted
html: 2026-09-25-usage-metrics-api-adds-pull-request-review-stages.html
preview:
  file: 2026-09-25-usage-metrics-api-adds-pull-request-review-stages.preview-9e5fd12aa6e5.webp
  width: 256
  height: 134
  color: '#08132e'
images:
- source: https://github.blog/wp-content/uploads/2026/09/6b8e111412cb6b98fa885fd25af7a5fa-2400x1260-1.jpeg
  original:
    file: 2026-09-25-usage-metrics-api-adds-pull-request-review-stages.image-0e439b0cd64e.jpg
    width: 2400
    height: 1260
  color: '#010409'
---

The enterprise and organization [repository-level Copilot usage metrics](https://github.blog/changelog/2026-07-17-repository-level-github-copilot-usage-metrics-generally-available/) reports now break down how long pull requests spend in each stage of review. A new `pull_request_review_times` array on each `repos-1-day` row reports a median and a 90th percentile for the time from ready for review to first review, first review to final review, and final review to merge.

### [What’s new](https://github.blog/changelog/2026-09-25-usage-metrics-api-adds-pull-request-review-stages#whats-new)

Each entry in `pull_request_review_times` includes:

- **`authored_by` and `reviewed_by`**: Who opened and who reviewed the pull requests in this entry. Both are `human` in this release.
- **`total_merged`**: The number of qualifying pull requests merged in the repository that day.
- **`median_minutes_ready_to_first_review` and `p90_minutes_ready_to_first_review`**: Time from the pull request becoming ready for review to its first review.
- **`median_minutes_first_to_final_review` and `p90_minutes_first_to_final_review`**: Time between the first and final review.
- **`median_minutes_final_review_to_merge` and `p90_minutes_final_review_to_merge`**: Time from the final review to merge.

Durations are in minutes and attributed to the day the pull request merged. The existing `pull_requests` fields are unchanged.

### [Why this matters](https://github.blog/changelog/2026-09-25-usage-metrics-api-adds-pull-request-review-stages#why-this-matters)

Teams can already see when pull requests take a long time to merge, but not where the time goes. Splitting the wait into three stages shows whether a pull request is waiting for someone to look at it, waiting on back-and-forth between reviewers, or sitting approved and unmerged. Each of those points to a different fix, and the 90th percentile beside the median shows when a handful of slow pull requests is driving the delay.

### [Important notes](https://github.blog/changelog/2026-09-25-usage-metrics-api-adds-pull-request-review-stages#important-notes)

- **Availability:** Present in the enterprise and organization `repos-1-day` reports.
- **What is counted:** Pull requests that a person opened and at least one other person reviewed. Only human reviews are timed. Reviews from Copilot code review, other bots, and the author are ignored, so a pull request reviewed by both a person and Copilot code review is still included. As a result, `pull_request_review_times[].total_merged` is usually lower than `pull_requests.total_merged`, which also counts pull requests merged without any reviews.
- **No backfill:** Data builds forward from the release date, so early days will be thin. Pull requests that became ready for review before September 21, 2026 are left out of this section but still count toward `pull_requests.total_merged`.
- **A quiet day is an empty array, not a zero:** The array is `[]` on days a repository merged no qualifying pull requests. The first-to-final review stage is `0` when pull requests receive a single review.
- **Access:** Enterprise owners and billing managers, organization owners, and anyone with a custom organization or enterprise role that grants the `View Copilot Metrics` permission can access these reports. The Copilot usage metrics policy must be enabled.

Visit the [Copilot usage metrics API documentation](https://docs.github.com/rest/copilot/copilot-usage-metrics) to get started.
