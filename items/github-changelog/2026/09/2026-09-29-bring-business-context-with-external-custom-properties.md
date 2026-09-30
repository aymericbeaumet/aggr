---
title: Bring business context with external custom properties
link: https://github.blog/changelog/2026-09-29-bring-business-context-with-external-custom-properties
source: github-changelog
published: 2026-09-29T18:40:27Z
updated: 2026-09-29T18:40:27Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- Allison
labels:
- platform governance
- release
summary: You can now seamlessly bring business context about your repositories from an external system of record (e.g., a configuration management database (CMDB), an internal developer portal, or an in-house system)… The post Bring business context with external custom properties appeared first on The GitHub Blog.
content: extracted
html: 2026-09-29-bring-business-context-with-external-custom-properties.html
preview:
  file: 2026-09-29-bring-business-context-with-external-custom-properties.preview-d9df00233cc1.webp
  width: 256
  height: 134
  color: '#13181c'
images:
- source: https://github.blog/wp-content/uploads/2026/09/657660083-b1c102e4-31bb-4066-87c9-535fa270cbd1.jpg
  original:
    file: 2026-09-29-bring-business-context-with-external-custom-properties.image-9560510421c6.jpg
    width: 2400
    height: 1260
  color: '#02050a'
---

You can now seamlessly bring business context about your repositories from an external system of record (e.g., a configuration management database (CMDB), an internal developer portal, or an in-house system) into GitHub with external custom properties. This feature is now in public preview.

Keep business context aligned with its source, with updates managed by your external system. This includes information such as ownership, service tier, lifecycle stage, and compliance status.

## [When to choose external custom properties](https://github.blog/changelog/2026-09-29-bring-business-context-with-external-custom-properties#when-to-choose-external-custom-properties)

Use custom properties when you want to manage this context in GitHub, whether through the UI or APIs. Choose external custom properties when an external system should exclusively own and continuously update that context:

- **Read-only in the GitHub UI:** Users can’t edit these values in GitHub, preventing conflicting updates across systems.
- **Ongoing synchronization:** Your external integration uses the external custom properties APIs to keep values current as your source data changes.
- **Dedicated namespace:** Your integration manages properties under its own prefix, keeping them separate from properties managed by other sources.

You can use external custom properties everywhere you use custom properties today, including repository views, repository filtering, and ruleset targeting. Your existing governance can use the synced business context while your external system remains the source of truth.

## [Connect your system of record](https://github.blog/changelog/2026-09-29-bring-business-context-with-external-custom-properties#connect-your-system-of-record)

Port.io is the first partner to integrate with external custom properties. Read [Port.io’s announcement](https://www.port.io/blog/github-external-custom-properties) for an overview, or follow the [Port.io integration guide](https://docs.port.io/guides/all/sync-port-properties-to-github-external-custom-properties/) to sync your business context to GitHub.

You aren’t limited to partner integrations. Any enterprise can build its own integration using the external custom properties APIs, with fine-grained permissions to control its access. To get started, see [Integrating custom properties with an external system](https://docs.github.com/organizations/managing-organization-settings/sync-external-custom-properties).

Join the conversation in the [GitHub Community announcements category](https://github.com/orgs/community/discussions/categories/repositories).
