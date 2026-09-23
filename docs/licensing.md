# Licensing

aggr is source-available under the unmodified
[Functional Source License, Version 1.1, ALv2 Future License](../LICENSE)
(FSL-1.1-ALv2). The licensor is Aymeric Beaumet; the required copyright notice is
in [NOTICE](../NOTICE).

## Using aggr

You can run and modify aggr for any Permitted Purpose without a license fee or trial
deadline, provided you comply with the license. The license names internal use and
access, non-commercial education, non-commercial research, and professional services
you provide to another licensee. This includes commercial organizations using aggr
for themselves.

A Permitted Purpose is any purpose other than a Competing Use: making aggr available
to others in a commercial product or service that substitutes for it, substitutes for
another product or service the licensor already offers using it, or offers the same or
substantially similar functionality. A hosted reader marketed as a substitute for aggr
is the clear case. Read the Permitted Purpose section for the actual test, and note
that the Patents and Trademarks sections carry their own conditions.

Because of this restriction, aggr is **source-available, not OSI open source** on the
day a version ships. It does not stay that way: the license irrevocably grants an
additional Apache-2.0 license to each version, effective on the second anniversary of
the date that version was made available. On or after that date you may use that
version under Apache-2.0, competing use included, and nothing the licensor does later
can withdraw it.

The purpose is to keep personal and internal company use free, and to let the author
develop a commercial hosted service, without the code ever becoming permanently
closed. For a commercial license before a version's Apache-2.0 date, contact
[hi@aymericbeaumet.com](mailto:hi@aymericbeaumet.com).

This page explains the choice; [the license text](../LICENSE) governs. It is the
[published FSL-1.1-ALv2 template](https://fsl.software/) with only its copyright
notice filled in, and no custom restrictions or exceptions.

## Earlier releases and third-party material

Every version published so far, up to and including
[v1.9.0](https://github.com/aymericbeaumet/aggr/blob/v1.9.0/LICENSE), was released
under MIT and remains available under those terms. This change does not revoke those
permissions or prevent someone from commercially using an earlier MIT version. Check
the license shipped with the exact revision you use.

Dependencies and vendored assets retain their own licenses. The browser client is
first-party with no third-party code; the bundled font keeps its adjacent license
file. `cargo deny check` checks Rust dependency licenses against
[deny.toml](../deny.toml).

The software license does not give you rights to fetched articles or images, or
license your own content to the project. See [hosting](hosting.md) before publishing
an archive.

## Contributions and distribution

Original contributions use the standard MIT inbound license, with explicit
acknowledgement as described in [CONTRIBUTING.md](../CONTRIBUTING.md). Contributors
keep copyright, and their individual contributions remain MIT-licensed. This allows
the maintainer to offer the combined project under FSL-1.1-ALv2 or a separate
commercial license without requiring contributors to assign copyright.

When distributing aggr, keep its license and required notice, together with the
notices and terms for third-party material and MIT contributions. The license's
Redistribution section requires a copy of or a link to its terms with any copy,
modification or derivative, and that copyright notices stay in place. Release archives
include `LICENSE`, `NOTICE`, and `LICENSE-CONTRIBUTIONS` alongside the binary.
