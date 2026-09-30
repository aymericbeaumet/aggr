---
title: 'Be alert: targeted attacks on prominent Rustaceans'
link: https://blog.rust-lang.org/2026/09/17/targeted-attacks/
source: rust-blog
published: 2026-09-17T00:00:00Z
updated: 2026-09-17T00:00:00Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- Adam Harvey
content: extracted
html: 2026-09-17-be-alert-targeted-attacks-on-prominent-rustaceans.html
preview:
  file: 2026-09-17-be-alert-targeted-attacks-on-prominent-rustaceans.preview-202cd0284a4a.webp
  width: 256
  height: 128
  color: '#b2b2b2'
images:
- source: https://www.rust-lang.org/static/images/rust-social-wide.jpg
  original:
    file: 2026-09-17-be-alert-targeted-attacks-on-prominent-rustaceans.image-577295821d7f.jpg
    width: 2048
    height: 1024
  color: '#fefefe'
---

We believe that there is an ongoing campaign targeting rust-lang members and owners of popular crates that is attempting to compromise devices and accounts in order to use them to publish malware.

## What we've seen

A video call is set up for something positive — maybe for a job, maybe for a project, maybe for a contract opportunity — and then that's used as a vector to either get the target to install something on their computer (such as a purportedly missing audio codec) or execute another command (for example, via putting a command on the clipboard).

These attackers are setting up new but legitimate seeming company profiles, including plausible LinkedIn presences, in order to pass cursory inspection.

A [previous attack of this form](https://grack.com/blog/2026/06/25/dissecting-a-failed-nation-state-attack/) targeted many prominent Rust developers in June, and, last month, the [`arrayref` crate was briefly compromised through similar attacks](https://blog.rust-lang.org/2026/08/20/supply-chain-attack-on-arrayref/). At this moment we do not know if these are all a part of the same campaign.

This attack style is [known to be used by the DPRK](https://kudelskisecurity.com/research/how-dprks-contagious-interview-campaign-targets-developers), and has been [seen outside of the Rust community as well](https://ashishb.net/security/contagious-interview/).

## What you can do

Please take extra care in the near term. Be appropriately suspicious of cold outreaches, and ensure that any calls you have with new people are on platforms you trust — ideally, try to be the one who sets up the call on a platform you already use.

Please also re-check that your accounts look normal: MFA enabled, no unexpected logins on platforms that can track that, and so on.

If you have any concerns about your accounts, please reach out to [help@crates.io](mailto:help@crates.io) (for crates.io account concerns) and/or [security@rust-lang.org](mailto:security@rust-lang.org) (for any other concerns). We're very happy to help.
