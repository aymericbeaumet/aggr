---
title: Turbo Haskell
link: https://comonad.com/reader/2026/turbo-haskell/
source: lobsters
published: 2026-09-30T13:35:52Z
updated: 2026-09-30T13:35:52Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- comonad.com via carbolymer
labels:
- haskell
- java
- programming
- vibecoding
summary: Comments
content: extracted
html: 2026-09-30-turbo-haskell.html
---

Exactly a week ago (as a joke), I started writing THC, my “Turbo Haskell compiler,” while on vacation visiting [Bartosz Milewski](https://bartoszmilewski.com/).

It has grown a tiny bit since then.

THC now implements every one of GHC 9.14.1’s prim-ops and provides a JIT for GHC Core that runs Haskell on the JVM. It uses the approach for running typed functional languages I developed several years ago in [Cadenza](https://github.com/ekmett/cadenza) ([talk](https://www.youtube.com/watch?v=gbmURWs_SaU)), using [Truffle](https://www.graalvm.org/latest/graalvm-as-a-platform/language-implementation-framework/) and [GraalVM](https://www.graalvm.org/).

GHC still handles parsing, typechecking, desugaring, and Core optimization. THC takes over from there, compiling and executing that Core through its own runtime on Truffle/GraalVM. Advanced language features such as Template Haskell and Linear Haskell are fully supported.

While it can be used as a JIT for GHC-grade Haskell, it also supports ahead-of-time (AOT) compilation with Native Image, allowing it to produce executables.

THC is capable of JIT- or AOT-compiling a number of Haskell programs, including `pandoc`, `happy`, `alex`, and, as of today, even GHC itself!

THC resolves packages using Cabal and fully supports packages with multiple libraries, including Backpack.

## Borrowing libraries

THC provides polyglot FFI to [Python](https://github.com/oracle/graalpython "GraalPy"), [Ruby](https://github.com/truffleruby/truffleruby "TruffleRuby"), [R](https://github.com/oracle/fastr "FastR"), and [JavaScript](https://github.com/oracle/graaljs "GraalJS"), letting Haskell raid libraries from other languages and bring their output straight into a JIT-compiled Haskell program. Conversion between `Data.Text` and Truffle strings over FFI is zero-copy for UTF-8-encoded strings inside other polyglot languages.

The idea is that if you need a data frame, want to run an LLM, or want a D3.js visualization, you should just pass `Text` out through foreign imports. Quasi-quotation-based `inline-<language name>` style bindings should be pretty easy to implement as well.

C/C++ bits in your Haskell libraries are run via FFI to native-mode [Sulong](https://www.graalvm.org/latest/reference-manual/llvm/) (LLVM on the JVM). Managed-mode Sulong, where LLVM is interpreted inside the JVM and pointers are managed and garbage-collected, is also available, but is not used by the normal `foreign import` path.

## Evaluation and concurrency

Internally, THC supports two different backends for Truffle evaluation: a bytecode-based JIT target and a traditional AST-based JIT target. Both can run in a single-threaded or multi-threaded style, with additional locking for the latter. It also supports GHC bytecode itself, so it can run BCO code as produced by GHCi.

THC fully supports `throwTo`, asynchronous exceptions that leave behind resumable code, and masking.

THC supports both “normal” Java threading and [Project Loom](https://openjdk.org/projects/loom/), upon which it offers lightweight GHC-style green threading with a HEC-style runtime executor permitting cheap `MVar`s and the like.

## SIMD

THC supports SIMD using the relatively limited supply of available GHC prim-ops, but it can also go further, allowing runtime selection of the SIMD “species” width and JIT-compiling loops using that information through the incubating [Vector API](https://openjdk.org/jeps/508 "JEP 508: Vector API (Tenth Incubator), JDK 25") (`jdk.incubator.vector`). This lets the JIT start to earn its keep!

In fact, nothing prevents the runtime from providing complete `RuntimeRep`-polymorphic code at runtime other than the fact that we have no Core that takes advantage of that freedom!

## Tail calls

Hot tail calls become loops. When execution has to fall back to ordinary calls, THC periodically unwinds the accumulated stack frames.

Previous efforts to run functional code on the JVM, such as the [Eta programming language](https://eta-lang.org/) and the design I used with Runar Bjarnason for trampolining Scalaz’s monads, used a trampoline mechanism. THC instead uses a code transformation trick to keep hot tail calls inside tight basic-block-style loops with side exits.

During recursion in tracing mode, THC fills a 64-bit [Bloom filter](https://en.wikipedia.org/wiki/Bloom_filter) to detect likely recursive tail calls. When it finds a likely hit, it throws a slow-path exception to connect the continuation with the launch site, and then custom Truffle nodes get Graal to transform the current tail-call loop across function bodies into a tight loop. False positives mean extra slow-path work; they don’t change the program’s result.

When later code paths diverge, THC tries to grow additional side loops, like a tracing JIT, until it hits the JVM’s limits on function body size. At that point, it’ll finally spill a tail call in a way that “leaks” a stack frame.

That leak is temporary. We can compact the accumulated stack frames using a trick somewhat similar to [CHICKEN Scheme’s garbage collection strategy](https://www.more-magic.net/posts/internals-gc.html), reusing the machinery we needed to support resumable code in the presence of asynchronous exceptions. The result is that hot loops can run *very* hot indeed.

When benchmarking `Data.Map` in particular, I found it needed something like 66 fallback trampoline calls compared to several million fast-path calls.

## Performance

The runtime can also use [compressed ordinary object pointers (compressed oops)](https://docs.oracle.com/en/java/javase/25/vm/java-hotspot-virtual-machine-performance-enhancements.html). These represent heap references as 32-bit offsets rather than full 64-bit pointers, reducing the memory occupied by references and helping more data fit in cache. With the JVM’s usual 8-byte object alignment, this limits the heap to roughly 32 GB when running in this mode.

Performance was a key consideration for the first couple of days of development. For tests on `Data.Map` and the like, I was able to get things to run within a general range of 3× faster to 3× slower after warmup, mostly hovering around 10–20% slower than GHC. That said, we haven’t been benchmarking for the last few days while we raced for broader coverage and suffered a 10× or so performance regression on some easy benchmarks in the meantime. Development effort continues to plug away at these to keep it under control.

We haven’t yet tested whether stack growth on non-tail-call paths remains bounded relative to GHC’s stack usage. Ensuring that bound remains possible future work.

## Development

The code is available at [github.com/ekmett/thc](https://github.com/ekmett/thc), with [documentation](https://ekmett.github.io/thc/) covering how to build, run, and use THC.

Development is proceeding on `irc.libera.chat` in the [##thc channel](https://web.libera.chat/##thc).

Come join us.

—Edward Kmett

[Discuss on Reddit](https://www.reddit.com/r/haskell/comments/1wu24oe/turbo_haskell/).
