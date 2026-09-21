# Licensing

aggr is source-available under the unmodified
[PolyForm Perimeter License 1.0.1](../LICENSE). The licensor is Aymeric Beaumet;
the required copyright notice is in [NOTICE](../NOTICE).

## Using aggr

You can run and modify aggr for your own use or your company's internal use without
a license fee or trial deadline, provided you comply with the license. This includes
commercial organizations. The license contains termination provisions for violations;
it is not an unconditional promise of access to future versions or hosted services.

The restriction is providing others a competing product or service, including a
hosted reader marketed as a substitute for aggr. The restriction applies even when
that competing offering is free. It is broader than a ban on paid resale, and is
not a blanket ban on every commercial use or sale. Read the license's Noncompete
and Competition sections for the actual test.

Because of this restriction, aggr is **source-available, not OSI open source**.
The purpose is to keep personal and internal company use free while allowing the
author to develop a commercial hosted service. For a separate commercial license,
contact [hi@aymericbeaumet.com](mailto:hi@aymericbeaumet.com).

This page explains the choice; [the license text](../LICENSE) governs. It follows
the [published PolyForm Perimeter 1.0.1 text](https://polyformproject.org/licenses/perimeter/1.0.1)
without custom restrictions or exceptions.

## Earlier releases and third-party material

Versions published under MIT, including [v1.9.0](https://github.com/aymericbeaumet/aggr/blob/v1.9.0/LICENSE)
and earlier releases, remain available under their original MIT terms. This change
does not revoke those permissions or prevent someone from commercially using an
earlier MIT version. Check the license shipped with the exact revision you use.

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
keep copyright, and their individual contributions remain MIT-licensed. This
allows the maintainer to offer the combined project under Perimeter or a separate
commercial license without requiring contributors to assign copyright.

When distributing aggr, keep its license and required notice, together with the
notices and terms for third-party material and MIT contributions. Release archives
include `LICENSE`, `NOTICE`, and `LICENSE-CONTRIBUTIONS` alongside the binary.
The generated JavaScript and component stylesheet also retain the required notice
and the Perimeter license URL in a comment; rebuilding the client regenerates these
comments and `client.LICENSE` from the repository's license files.
