---
title: Self-hosted runner version enforcement date has moved
link: https://github.blog/changelog/2026-09-28-self-hosted-runner-version-enforcement-date-has-moved
source: github-changelog
published: 2026-09-28T19:22:46Z
updated: 2026-09-28T19:22:46Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- Allison
labels:
- actions
- retired
summary: The enforcement date for GitHub Actions minimum version requirements for self-hosted runners on GitHub Enterprise Cloud has changed. The change ships Monday, September 28, 2026, and full enforcement now begins… The post Self-hosted runner version enforcement date has moved appeared first on The GitHub Blog.
content: extracted
html: 2026-09-28-self-hosted-runner-version-enforcement-date-has-moved.html
preview:
  file: 2026-09-28-self-hosted-runner-version-enforcement-date-has-moved.preview-216750e28dae.webp
  width: 256
  height: 134
  color: '#1c0f14'
images:
- source: https://github.blog/wp-content/uploads/2026/09/658133509-427475fd-0c8b-4903-9db2-d4d6056635dd.jpeg
  original:
    file: 2026-09-28-self-hosted-runner-version-enforcement-date-has-moved.image-002a89a1fb27.jpg
    width: 2400
    height: 1260
  color: '#020409'
---

The enforcement date for [GitHub Actions minimum version requirements for self-hosted runners](https://github.blog/changelog/2026-06-12-github-actions-minimum-version-enforcement-timeline-for-self-hosted-runners/) on GitHub Enterprise Cloud has changed. The change ships Monday, September 28, 2026, and full enforcement now begins Tuesday, September 29, 2026. This is instead of the date that was previously announced.

The requirements themselves are unchanged:

- Self-hosted runners below version `2.329.0` won’t be able to register or reregister.
- Existing runners below the minimum version required to execute workflow jobs (which is a higher version than the registration minimum) will stop running jobs, even if they were previously registered.

If you haven’t upgraded your self-hosted runners yet, do so before September 29, 2026 to avoid disruption to your workflows. See the [self-hosted runner documentation](https://docs.github.com/actions/hosting-your-own-runners) for upgrade guidance. You can also use the new [REST API for runner version deprecations](https://github.blog/changelog/2026-09-03-github-actions-early-september-2026-updates/#new-rest-api-for-runner-version-deprecations) to check the registration and runtime deprecation dates for any runner version and build automated alerts for fleets nearing their deadline.

This shift only affects GitHub Enterprise Cloud. GitHub Enterprise Server isn’t impacted. Enforcement already began for GitHub Enterprise Cloud with Data Residency on July 31, 2026.
