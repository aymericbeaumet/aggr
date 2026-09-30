---
title: Announcing Rust 1.98.1
link: https://blog.rust-lang.org/2026/09/03/Rust-1.98.1/
source: rust-blog
published: 2026-09-03T00:00:00Z
updated: 2026-09-03T00:00:00Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- The Rust Release Team
content: extracted
html: 2026-09-03-announcing-rust-1-98-1.html
preview:
  file: 2026-09-03-announcing-rust-1-98-1.preview-202cd0284a4a.webp
  width: 256
  height: 128
  color: '#b2b2b2'
images:
- source: https://www.rust-lang.org/static/images/rust-social-wide.jpg
  original:
    file: 2026-09-03-announcing-rust-1-98-1.image-577295821d7f.jpg
    width: 2048
    height: 1024
  color: '#fefefe'
---

The Rust team has published a new point release of Rust, 1.98.1. Rust is a programming language that is empowering everyone to build reliable and efficient software.

If you have a previous version of Rust installed via rustup, getting Rust 1.98.1 is as easy as:

```plain
rustup update stable
```

If you don't have it already, you can [get `rustup`](https://www.rust-lang.org/install.html) from the appropriate page on our website.

## What's in 1.98.1

Rust 1.98.1 fixes a [miscompilation in vtable generation](https://github.com/rust-lang/rust/issues/161441).

In Rust 1.98.0, in some circumstances, rustc would incorrectly generate a trait object vtable with a null pointer where a function pointer should be. This leads to undefined behavior in the emitted code. In some cases this may 'just' cause segfaults due to the null pointer being loaded, but it is possible for it to be justification for arbitrary effects (as is typical for UB).

If you'd like to help us out by testing future releases, you might consider using the beta (`rustup default beta`) and nightly (`rustup default nightly`) channels locally and in your CI. Please [report](https://github.com/rust-lang/rust/issues/new/choose) any bugs you might come across!

### Contributors to 1.98.1

Many people came together to create Rust 1.98.1. We couldn't have done it without all of you. [Thanks!](https://thanks.rust-lang.org/rust/1.98.1/)
