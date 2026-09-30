---
title: Repository custom runner settings for Dependabot
link: https://github.blog/changelog/2026-09-29-repository-custom-runner-settings-for-dependabot
source: github-changelog
published: 2026-09-29T19:10:00Z
updated: 2026-09-29T19:10:00Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- Allison
labels:
- improvement
- supply chain security
summary: As a repository administrator, you can now configure the runner type, optional custom label, and optional runner group for Dependabot version and security updates. This extends the runner configuration already… The post Repository custom runner settings for Dependabot appeared first on The GitHub Blog.
content: extracted
html: 2026-09-29-repository-custom-runner-settings-for-dependabot.html
preview:
  file: 2026-09-29-repository-custom-runner-settings-for-dependabot.preview-945e6da52a42.webp
  width: 256
  height: 134
  color: '#0e1833'
images:
- source: https://github.blog/wp-content/uploads/2026/09/Changelog_Improvement_Unfurl_TextOnly_RepositoryCustomRunner.jpg
  original:
    file: 2026-09-29-repository-custom-runner-settings-for-dependabot.image-9fc17d5f90b7.jpg
    width: 2400
    height: 1260
  color: '#010409'
- source: https://github.com/user-attachments/assets/789b8f34-1a90-4ce1-af99-ac864321fc6e
  original:
    file: 2026-09-29-repository-custom-runner-settings-for-dependabot.image-340139623e37.jpg
    width: 2400
    height: 1260
  color: '#010409'
---

As a repository administrator, you can now configure the runner type, optional custom label, and optional runner group for Dependabot version and security updates. This extends the [runner configuration already available at the organization level](https://github.blog/changelog/2025-11-25-custom-labels-configuration-option-for-dependabot-self-hosted-and-larger-github-hosted-actions-runners-now-generally-available-at-the-organization-level/), giving you more control over where each repository’s Dependabot jobs run.

Labeled runners can target both self-hosted and larger GitHub-hosted runners suited to your project’s needs, such as access to private package registries or specialized environments.

These repository-level settings are available for private and internal repositories on github.com. The controls are hidden for public repositories and on GitHub Enterprise Server.

To get started, open your repository settings and select **Advanced Security**. Under “Dependency scanning”, find “Dependabot version updates”, then edit **Runner type**. Choose **Labeled runner**, then optionally enter a custom label and runner group. If you do not specify a label, Dependabot uses the `dependabot` label. Alternatively, choose **Standard GitHub runner** to use the default GitHub-hosted environment.

Security configurations do not currently enforce Dependabot runner settings.

Check out the docs to learn more about [using custom labels with self-hosted runners](https://docs.github.com/actions/how-tos/manage-runners/self-hosted-runners/apply-labels) and [managing Dependabot on self-hosted runners](https://docs.github.com/code-security/how-tos/secure-your-supply-chain/manage-your-dependency-security/configure-on-self-hosted-runners).

![social](https://github.com/user-attachments/assets/789b8f34-1a90-4ce1-af99-ac864321fc6e)
