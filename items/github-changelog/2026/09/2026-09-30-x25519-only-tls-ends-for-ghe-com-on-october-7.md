---
title: X25519-only TLS ends for GHE.com on October 7
link: https://github.blog/changelog/2026-09-30-x25519-only-tls-ends-for-ghe-com-on-september-15
source: github-changelog
published: 2026-09-30T13:30:24Z
updated: 2026-09-30T13:30:24Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- Allison
labels:
- application security
- ecosystem &amp; accessibility
- enterprise management tools
- retired
summary: Beginning October 7, 2026, GitHub Enterprise Cloud with data residency will no longer accept TLS connections from clients that offer only X25519 for key agreement. Most customers don’t need to… The post X25519-only TLS ends for GHE.com on October 7 appeared first on The GitHub Blog.
content: extracted
html: 2026-09-30-x25519-only-tls-ends-for-ghe-com-on-october-7.html
preview:
  file: 2026-09-30-x25519-only-tls-ends-for-ghe-com-on-october-7.preview-8abcbcd9496d.webp
  width: 256
  height: 134
  color: '#1d0e14'
images:
- source: https://github.blog/wp-content/themes/github-2021-child/dist/img/social-v3-deprecations.jpg
  original:
    file: 2026-09-30-x25519-only-tls-ends-for-ghe-com-on-october-7.image-c821388432f3.jpg
    width: 1200
    height: 629
  color: '#030409'
---

Beginning October 7, 2026, GitHub Enterprise Cloud with data residency will no longer accept TLS connections from clients that offer only X25519 for key agreement.

Most customers don’t need to take action. The affected endpoints will continue to support the FIPS-approved P-256 (`secp256r1`) and P-384 (`secp384r1`) groups. Current browsers, operating systems, GitHub CLI releases, and commonly used TLS libraries already support P-256.

You may be affected if an application, proxy, security appliance, or TLS library is explicitly configured to offer only X25519. Before October 7, 2026, you should:

- Update your operating system, runtime, GitHub CLI, proxy, and TLS libraries to supported versions.
- Remove any X25519-only configuration.
- Ensure P-256 (`secp256r1`) is enabled. You may also enable P-384 (`secp384r1`).

After October 7, X25519-only clients will be unable to establish HTTPS connections. This change applies only to GitHub Enterprise Cloud with data residency. SSH connectivity is not affected.

If you need help validating your TLS configuration, [contact GitHub Support](https://support.github.com/contact).
