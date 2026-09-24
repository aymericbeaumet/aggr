# Licensing

aggr is open source under the unmodified [MIT License](../LICENSE). The copyright
holder is Aymeric Beaumet.

## Using aggr

You can run, modify, distribute and sell aggr, commercially or not, as long as the
copyright notice and the permission notice in [LICENSE](../LICENSE) travel with every
copy or substantial portion of the software. There is no field-of-use restriction, no
trial period, and no separate commercial license to buy. The software is provided as
is, without warranty.

Every version published so far carries these terms, and so does every revision in this
repository. Check the license shipped with the exact revision you use.

## Third-party material

Dependencies and vendored assets retain their own licenses. The browser client is
first-party with no third-party code; the bundled font keeps its adjacent license file.
`cargo deny check` checks Rust dependency licenses against [deny.toml](../deny.toml),
whose allow-list is exhaustive so that copyleft code cannot enter the binary
unnoticed.

The software license does not give you rights to fetched articles or images, or
license your own content to the project. See [hosting](hosting.md) before publishing
an archive.

## Contributions and distribution

Original contributions arrive under the same MIT terms, with explicit acknowledgement
as described in [CONTRIBUTING.md](../CONTRIBUTING.md). Contributors keep copyright;
inbound and outbound terms are identical, so no copyright assignment is required.

When distributing aggr, keep `LICENSE` with it, together with the notices and terms
for third-party material.
