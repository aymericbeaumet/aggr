---
title: 'attezt: device attestation, PKCS11 and ACME'
link: https://media.ccc.de/v/all-systems-go-2026-414-attezt-device-attestation-pkcs11-and-acme#t=30
source: lobsters
published: 2026-09-30T12:23:08Z
updated: 2026-09-30T12:23:08Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- media.ccc.de via hoistbypetard
labels:
- security
- video
summary: Comments
content: extracted
html: 2026-09-30-attezt-device-attestation-pkcs11-and-acme.html
preview:
  file: 2026-09-30-attezt-device-attestation-pkcs11-and-acme.preview-d29ec68ff2be.webp
  width: 256
  height: 144
  alt: 'attezt: device attestation, PKCS11 and ACME'
  color: '#434853'
images:
- source: https://static.media.ccc.de/media/events/all_systems_go/2026/414-23d6c6b3-8a6b-5a07-8531-d9bab98c051b_preview.jpg
  original:
    file: 2026-09-30-attezt-device-attestation-pkcs11-and-acme.image-976f2fb9e728.jpg
    width: 1920
    height: 1080
  color: '#272735'
---

1. [browse](https://media.ccc.de/b)
2. [conferences](https://media.ccc.de/b/conferences)
3. [all\_systems\_go](https://media.ccc.de/b/conferences/all_systems_go)
4. [asg2026](https://media.ccc.de/b/conferences/all_systems_go/asg2026)
5. event

[Morten Linderud](https://media.ccc.de/search?p=Morten+Linderud)

[asg2026-eng](https://media.ccc.de/c/asg2026/asg2026-eng) [Galerie](https://media.ccc.de/c/asg2026/Galerie) Playlists: ['asg2026' videos starting here](https://media.ccc.de/v/all-systems-go-2026-414-attezt-device-attestation-pkcs11-and-acme/playlist) / [audio](https://media.ccc.de/v/all-systems-go-2026-414-attezt-device-attestation-pkcs11-and-acme/audio)

IETF is standardizing a new ACME challenge, \`device-attest-01\`, which allows organizations to provision device bound certificates to machines, and enables machines to present signing certificates that can't be extracted out of machines they where intended for. This is useful for cases where you want to provide reverse proxies with mTLS with a strong sense of device identity.

attezt is intended to be a suite of tools to work the new \`device-attest-01\` ACME challenges for Linux. It provides an ACME client, an attestation server with (simple) support for inventory systems and a PKCS11 agent which together enables the support of this ACME challenge on Linux.

This talk will give an introduction to the new ACME challenge, a quick rundown of how an attestation server works and how attezt works.

https://datatracker.ietf.org/doc/draft-ietf-acme-device-attest/ \
https://github.com/Foxboron/attezt

Licensed to the public under https://creativecommons.org/licenses/by/4.0/de/

### Download

#### Audio

### Tags
