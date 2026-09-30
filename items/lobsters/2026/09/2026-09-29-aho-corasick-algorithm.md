---
title: Aho-Corasick Algorithm
link: https://compiler.club/aho-corasick/
source: lobsters
published: 2026-09-29T14:49:08Z
updated: 2026-09-29T14:49:08Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- compiler.club via abhin4v
labels:
- compsci
summary: Comments
content: extracted
html: 2026-09-29-aho-corasick-algorithm.html
preview:
  file: 2026-09-29-aho-corasick-algorithm.preview-75536214a632.webp
  width: 256
  height: 47
  alt: example trie
  color: '#f3f3f3'
images:
- source: https://compiler.club/img/aho-corasick/trie.svg
  original:
    file: 2026-09-29-aho-corasick-algorithm.image-224380b157bb.png
    width: 924
    height: 168
  variants:
  - file: 2026-09-29-aho-corasick-algorithm.image-0fa7d33267d7.webp
    width: 320
    height: 58
  - file: 2026-09-29-aho-corasick-algorithm.image-cad6e5adfe94.webp
    width: 640
    height: 116
  - file: 2026-09-29-aho-corasick-algorithm.image-86d4c0de37e5.webp
    width: 924
    height: 168
  color: '#fcfcfc'
- source: https://compiler.club/img/aho-corasick/first.svg
  original:
    file: 2026-09-29-aho-corasick-algorithm.image-5d243b1a1803.png
    width: 814
    height: 392
  variants:
  - file: 2026-09-29-aho-corasick-algorithm.image-0e3f04255273.webp
    width: 320
    height: 154
  - file: 2026-09-29-aho-corasick-algorithm.image-6540c6974764.webp
    width: 640
    height: 308
  - file: 2026-09-29-aho-corasick-algorithm.image-083af2ff1914.webp
    width: 814
    height: 392
  color: '#fefdfd'
- source: https://compiler.club/img/aho-corasick/two.svg
  original:
    file: 2026-09-29-aho-corasick-algorithm.image-5da962fb6ddd.png
    width: 830
    height: 232
  variants:
  - file: 2026-09-29-aho-corasick-algorithm.image-4eab742740d6.webp
    width: 320
    height: 89
  - file: 2026-09-29-aho-corasick-algorithm.image-6f0d934671bd.webp
    width: 640
    height: 179
  - file: 2026-09-29-aho-corasick-algorithm.image-37a866b6e39d.webp
    width: 830
    height: 232
  color: '#fdfcfc'
- source: https://compiler.club/img/aho-corasick/three.svg
  original:
    file: 2026-09-29-aho-corasick-algorithm.image-3b8910a0499c.png
    width: 1067
    height: 243
  variants:
  - file: 2026-09-29-aho-corasick-algorithm.image-d1c2e712c55b.webp
    width: 320
    height: 73
  - file: 2026-09-29-aho-corasick-algorithm.image-81839406bf07.webp
    width: 640
    height: 146
  - file: 2026-09-29-aho-corasick-algorithm.image-da6464896171.webp
    width: 960
    height: 219
  - file: 2026-09-29-aho-corasick-algorithm.image-8863f12110f4.webp
    width: 1067
    height: 243
  color: '#fcfbfb'
- source: https://compiler.club/img/aho-corasick/four.svg
  original:
    file: 2026-09-29-aho-corasick-algorithm.image-ded16f7b604b.png
    width: 1147
    height: 206
  variants:
  - file: 2026-09-29-aho-corasick-algorithm.image-4e0b2d0df13d.webp
    width: 320
    height: 57
  - file: 2026-09-29-aho-corasick-algorithm.image-52d57fd7362d.webp
    width: 640
    height: 115
  - file: 2026-09-29-aho-corasick-algorithm.image-b4d549232a02.webp
    width: 960
    height: 172
  - file: 2026-09-29-aho-corasick-algorithm.image-0635c45d4b00.webp
    width: 1147
    height: 206
  color: '#fcfbfb'
- source: https://compiler.club/img/aho-corasick/five.svg
  original:
    file: 2026-09-29-aho-corasick-algorithm.image-6b3bcaa8b21d.png
    width: 718
    height: 307
  variants:
  - file: 2026-09-29-aho-corasick-algorithm.image-91e503c9936d.webp
    width: 320
    height: 137
  - file: 2026-09-29-aho-corasick-algorithm.image-309e04c523c6.webp
    width: 640
    height: 274
  - file: 2026-09-29-aho-corasick-algorithm.image-82e0f4b69bb0.webp
    width: 718
    height: 307
  color: '#fdfcfc'
- source: https://compiler.club/img/aho-corasick/before.svg
  original:
    file: 2026-09-29-aho-corasick-algorithm.image-de18c14ff08c.png
    width: 614
    height: 284
  variants:
  - file: 2026-09-29-aho-corasick-algorithm.image-7d552dcaf2c4.webp
    width: 320
    height: 148
  - file: 2026-09-29-aho-corasick-algorithm.image-8eb8478aaa39.webp
    width: 614
    height: 284
  color: '#fefdfd'
- source: https://compiler.club/img/aho-corasick/after.svg
  original:
    file: 2026-09-29-aho-corasick-algorithm.image-4fe0d37ee47b.png
    width: 788
    height: 686
  variants:
  - file: 2026-09-29-aho-corasick-algorithm.image-1b3fecd127be.webp
    width: 320
    height: 279
  - file: 2026-09-29-aho-corasick-algorithm.image-b5cf7d088ff9.webp
    width: 640
    height: 557
  - file: 2026-09-29-aho-corasick-algorithm.image-e407405d9bfb.webp
    width: 788
    height: 686
  color: '#fdfdfd'
---

### Introduction

This post describes the construction of an Aho-Corasick automaton for the simultaneous matching of substrings within a sequence. I’m fond of this algorithm because it constructs an automaton from an existing tree data structure in a rather pleasant way.

### Tries

A trie (or “prefix tree”) is an $n$-ary tree that stores a set of strings. Each edge in the trie is labelled with a character and each node conceptually represents the concatenation of all the edge characters required to reach it (starting from the root).

Tries are designed to reduce redundancy by ensuring entries with common prefixes share these prefixes within the tree data structure.

See the trie below that stores the strings ${\\lbrace \\text{suit}, \\text{suited}, \\text{suitable} \\rbrace}$:

![example trie](https://compiler.club/img/aho-corasick/trie.svg)

You can see that the common prefix of `suit` is shared by all entries. Also note that nodes representing complete entries in the trie are explicitly marked.

### Suffix Links

Aho-Corasick automatons recover from transition failure by following so-called suffix links. These links preserve the longest suffix of the string represented by each node that happens to exist as a prefix of a pattern in the trie. This allows the machine to transition to a state that permits further matching of patterns that happen to have the failing node’s longest suffix as a prefix.

Consider the suffix links applied (in red) to the trie constructed for the strings $\\lbrace \\text{item}, \\text{suits} \\rbrace$ below:

![example trie with suffix links](https://compiler.club/img/aho-corasick/first.svg)

The majority of nodes have the root node as their suffix link. However, if we look at node $8$, representing the state reached by following suit, we see that its suffix link (node $3$) points to the node one would reach if, starting from the root, we had followed its suffix, it. This permits the potential for matching the patterns prefixed with it; in this case, only $\\lbrace \\text{item} \\rbrace$.

For example, if we were scanning the input `suitems`, we would reach node $8$, see that there is no outgoing edge labelled `e`, we would transition via the suffix link and continue scanning from the failing character, eventually matching `su[item]s`.

The construction of suffix links is rather pleasant, they’re computed in a breadth-first traversal of the trie. The cases for each node are computed as follows:

- The root and its children have root as their suffix link.

- To compute the suffix link for each other node, you start by looking at the node’s parent’s suffix link. For a pattern of characters $( c\_1, c\_2, c\_3, \\ldots, c\_n)$, to compute a suffix link for a node $c\_k$ ($k \\geq 3$), you examine the suffix link of $c\_{k-1}$. That suffix link preserves the longest suffix of $(c\_1, \\ldots, c\_{k-1})$ that represents a prefix of a pattern in the trie. If the node at that suffix link has an outgoing edge labelled $c\_k$, then the target of that edge is $c\_k$’s suffix link. Otherwise, you continue to chase up the trie by following successive suffix links. If you reach the root, you stop (to avoid iterating indefinitely by following its suffix link to itself).

Despite my best efforts to describe the suffix link construction process above formally, it’s best explained visually. Consider the trie constructed for the strings $\\lbrace \\text{cadence}, \\text{facade} \\rbrace$ with partially constructed suffix links below:

![incremental example of suffix links](https://compiler.club/img/aho-corasick/two.svg)

The nodes are numbered with their breadth-first traversal order.

After processing the nodes with suffix links computed above, the BFS queue will contain $\[(c, 5), (d, 6)\]$. If we examine $(c, 5)$, we look at its parent’s suffix link. In this case, it’s the root of the trie. We see that root has an outgoing edge labelled with $c$, so the node reached by that edge is the suffix link for node $5$:

![updated incremental example of suffix links](https://compiler.club/img/aho-corasick/three.svg)

After the above, the queue will contain $\[(d, 6), (a, 7)\]$. The processing of $(d, 6)$ is straightforward. As before, its parent’s suffix link is the root. However, there is no outgoing edge labelled $d$, therefore node $6$’s suffix link is simply the root (capturing the idea that there’s no other pattern in the tree that has any of $\\lbrace \\text{c}, \\text{ca}, \\text{cad} \\rbrace$ as a prefix). In operational terms, there’s no suffix to preserve as another pattern’s prefix if we get to node $6$ and the input character is not $e$. We dispose of the seen $\\text{cad}$ and try the failing input character from the root. If a character fails to advance from the root, we stay at the root but advance the character stream (as it’s a non-starter for every pattern).

The next interesting case is that of processing the $(a, 7)$ edge. Node $7$’s parent’s suffix link is the previously computed, non-root, node $2$ - which does have an outgoing edge labelled $a$, therefore the suffix link of node $7$ is node $4$. This preserves the $\\text{ca}$ suffix of $\\text{faca}$ as being a valid prefix of the other pattern, $\\text{cadence}$.

When all nodes have been processed, the suffix links are as follows:

![final example of suffix links](https://compiler.club/img/aho-corasick/four.svg)

For clarity, I’ve omitted the suffix links that go to the root node. Interestingly, you can see that node $12$ goes to node $2$, attempting to preserve prefix context for matching $\\text{cadence}$, from a node reached by assuming it was making progress in matching $\\text{cadence}$!

### Output Links

When a pattern is introduced into the trie, the last node traversed during insertion is annotated as being an “output” node (usually storing the inserted pattern). If a node has an output pattern associated with it, this identifies a match that should be output when entering the state represented by that node. However, it may be the case that a node with an output’s pattern has a suffix that also happens to be a pattern in the trie - and, so, must also be output at the same time.

In order to capture this information, Aho-Corasick employs output links. As with suffix links, it will always be the case that output links point to nodes representing shorter strings (visited first in the breadth-first algorithm).

Consider the trie below (with suffix links in red and output links in blue), constructed from the strings $\\lbrace \\text{spin}, \\text{pin}, \\text{in} \\rbrace$.

![example of suffix links and output links](https://compiler.club/img/aho-corasick/five.svg)

The blue links go between node with an associated output. It’s clear that on reaching state $9$ (representing that `spin` has been matched), the outputs for $8$ and $6$ must also be output (representing pattern suffixes `pin` and `in`, respectively). You can think of output links as being a linked list of nodes that must be iteratively output if one was interpreting this matcher directly.

The output link for a node (if it has one) can be computed directly after its suffix link has been computed. This makes sense because the suffix link attempts to capture the longest suffix of the current node’s pattern that happens to be a prefix of a pattern in the trie. So, the output link for a node is its suffix node if its suffix node has an associated output (is a pattern itself), otherwise a node’s output is its suffix node’s output link.

### Algorithm Pseudocode

The pseudocode for computing suffix and output links is as follows:

let Q be a queue of (char, node)

```plain
# root and its immediate children have root as their suffix link
root.suffix <- root
for each (char, child) in root.arrows {
  child.suffix <- root

  # queue root's grandchildren for traversal
  add (char, child) to Q
}

while Q is not empty {
  let (char, node) = Q.poll()
  
  # start from parent's suffix link node
  let suffix = node.parent.suffix

  while suffix has no edge labelled char {
    # follow its suffix link
    suffix <- suffix.suffix

    # avoid looping endlessly if we reach root
    if suffix == root then
      break
  }

  # capture case where root has outgoing edge labelled char
  if suffix has edge (char, actual) then
    node.suffix <- actual
  else
    node.suffix <- suffix 
	
  # a node's output is its suffix if its suffix link is an output, 
  # otherwise follow its suffix's output
  if (node.suffix.pattern != null) then
    node.output = node.suffix
  else
    node.output = node.suffix.output
}
```

### The Automaton

The computation of suffix and output links is the core of Aho-Corasick, but only an intermediary step as far as computing the automaton is concerned. Of course, the trie - augmented with these internal links - can be interpreted directly. However, due to the potential to repetitively chase up suffix links to resolve the next state to transition to on a given symbol, the amount of work for each transition is not constant.

Once the suffix and output links are in place, the transitions and outputs are all statically resolvable into a deterministic finite automaton. To compute this, a final breadth first traversal is performed.

First, the root node (the base case) is processed. For every symbol, $a$, if the root node has an edge reaching some state, $s$, labelled $a$, then that’s where the root transitions on $a$. If no such edge for $a$ exists, then you stay in the same place (self-looping on the root - effectively skipping over the symbol as it’s a non-starter for every pattern in the trie). All states reachable from the root are added to the traversal queue during this processing.

For every other node processed in breadth-first fashion: for every symbol, $a$, you transition to the state reachable on an edge labelled $a$. If no such edge exists, then you transition to the state reached if you transitioned from the node’s suffix node. This encodes the idea that if no progress can be made on a certain path through the trie, then the state is transitioned into one that hopefully preserves the longest suffix of the current state that is also a prefix of a pattern in the trie. Often times, many transitions simply go to the root (preserving $\\epsilon$), so a sparse matrix storage representation is recommendable for many offline Aho-Corasick automatons.

For example, the trie with suffix and output links constructed from the strings $\\lbrace \\text{he}, \\text{she}, \\text{her} \\rbrace$ would be as follows:

![automaton before determinisation](https://compiler.club/img/aho-corasick/before.svg)

The DFA computed from the above trie (by resolving transitions and merging outputs into sets) would be:

![DFA computed from trie augmented with suffix and output links](https://compiler.club/img/aho-corasick/after.svg)

### Demo

Below you can build an Aho-Corasick trie from a set of strings:

### Further Reading

- [Efficient String Matching: An Aid to Bibliographic Search](https://cr.yp.to/bib/1975/aho.pdf) - original paper describing entire algorithm.
