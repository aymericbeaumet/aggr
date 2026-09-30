---
title: What TLA+ can and can't check
link: https://buttondown.com/hillelwayne/archive/what-tla-can-and-cant-check/
source: lobsters
published: 2026-09-30T14:02:33Z
updated: 2026-09-30T14:02:33Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- buttondown.com by hwayne
labels:
- formalmethods
summary: Comments
content: extracted
html: 2026-09-30-what-tla-can-and-can-t-check.html
preview:
  file: 2026-09-30-what-tla-can-and-can-t-check.preview-a7337ea31c16.webp
  width: 256
  height: 134
  color: '#e5e5e5'
images:
- source: https://image-generator.buttondown.email/api/emphasize-subject?subject=What%20TLA%2B%20can%20and%20can%27t%20check&author=Computer%20Things&date=2026-09-30&img=
  original:
    file: 2026-09-30-what-tla-can-and-can-t-check.image-1b55baf41e1b.png
    width: 600
    height: 315
  variants:
  - file: 2026-09-30-what-tla-can-and-can-t-check.image-034685d40b3d.webp
    width: 320
    height: 168
  - file: 2026-09-30-what-tla-can-and-can-t-check.image-11026a74dd15.webp
    width: 600
    height: 315
  color: '#fefefe'
---

Last week Boris Cherny, the inventor of Claude Code, mentioned that Opus was able to use TLA+[^1] to find race conditions in code.

And now everybody on the internet is talking about formal verification.

As a long-time educator ([1](https://link.springer.com/book/10.1007/978-1-4842-3829-5) [2](https://learntla.com/)) and [advocate](https://newsletter.pragmaticengineer.com/p/formal-methods-with-hillel-wayne) of TLA+, this is really exciting! TLA+ is great at designing complex concurrent systems and making sure they're bug-free.[^2] As a long-time advocate of level-headedness, this new euphoria worries me. I read a lot of people saying that formal methods will solve the problem of agentic software development once and for all, and that's nonsense.

Enough words have been spilled about the weaknesses of TLA+ in terms of what it can guarantee, like how [correct designs don't automatically translate into correct code](https://buttondown.com/hillelwayne/archive/what-if-the-spec-doesnt-match-the-code/). So I'd like to focus on a different limitation for this newsletter: to verify a property, we need to have a property to verify! So what are the properties that TLA+ can't even express?

(This assumes some basic knowledge of TLA+. If you're a total beginner, check out those \[1\] \[2\] things above or [read here](https://buttondown.com/hillelwayne/archive/tla-from-first-principles/).)

## What TLA+ can check

TLA+ divides the system into a set of behaviors. Each behavior is a sequence of states, like "light one is green, then yellow, then red". In each state we can express a regular boolean expressions like "Light four is green" or "All lights are red." We can also modify expressions with three "temporal" logical operators:

- `[]P` ("always P") is true if P is true in the current state *and every* future state. Example: `[](at_most_one_green)` is true if every state going forward has no more than one green light.
- `P'` ("P prime") is true if P is true in the next state. Example: `light="green" && light'="red"` is true if the light changes from green to red.
- `<>P` ("eventually P") is true if P is true in the current state *or in at least one* future state. Example: `<>(light4 = "yellow")` is true if light4 is yellow or is yellow in a future state.

When we say that P is a property of the system, we mean it is true in the initial state of every behavior. So if we check the property `[]P`, that means that `[]P` is true in every initial state, and then by the definition of "always" means that P is true in every future state from that initial state, meaning it is true in every state of every behavior. We call this an **invariant**, and is one of the most foundational properties we check in TLA+.

We can also compose `[]` with primes to get **action properties**, or change variants. `[](x' >= x)` is true if the new value of x is always greater than equal to the old value of x. Another fun one is `[](P => P')`: once P is true, it can never become false again. The actual TLA+ is a little more complicated because of a thing called "stutter-invariance", but that's extra details. Action properties and invariants are both **safety** properties, which roughly means "something bad never happens". [I wrote an article on safety and liveness here](https://www.hillelwayne.com/post/safety-and-liveness/).

Liveness, btw, is "something good always happens". All liveness properties are based off of `<>`. By itself, `<>P` just means "P is true in at least one state of every behavior", which is usually too weak to be a good system property. But with composition, we can make more interesting liveness properties:

- `[]<>P` is true if, in every state, P is true in at least one future state. This can represent recovery mechanisms like "If the nodes have a new leader election, they will eventually agree on a leader."
- `<>[]P` is true if, at some point in time, P becomes true and remains true forever. This is great for showing that algorithms terminate with the correct results.
- `[](P => <>Q)` is true if, for every state where P is true, there is a future state where Q is true. This can do things like show that P eventually causes Q, or that "all messages put on the queue are eventually in a reader's history." Parsing the formula is a little confusing, so we have the sugar `P ~> Q` (P leads to Q).

There's a couple of other operators, like `ENABLED` and `<<A>>_v`, which open up other tricks, but the majority of the stuff we check are invariants, action properties, and liveness. And [refinement](https://buttondown.com/hillelwayne/archive/refinement-without-specification/), which is a combination of safety and liveness and a topic unto itself.

## What TLA+ can't do

Let's start with the obvious one: if you don't know how to represent your property as a logical formula, then TLA+ can't help you. Nor can any formal method. If you can't formalize the human notion of a bird, you can't prove your app recognizes birds. And, unfortunately, a lot of important properties we care about fall into this category.

Next, the overly specific things. TLA+ safety properties work on the level of either individual states (invariants) or single step (action properties). You can't natively define a property over two or more steps, like "pressing `delete` and then `undo` gives you back the original state", or "once `power` is pressed, the computer turns on within ten steps". We also can't define properties on floating point operations or over [real time, just logical time](https://buttondown.com/hillelwayne/archive/physical-vs-logical-time/).

Now for limit that most interests me. TLA+ properties are implicitly quantified over all behaviors. I said that checking `[]P` means "P is true in every state," but what it actually means is "for all behaviors, `[]P` is true of that behavior's initial state." Any property TLA+ can check of the system must be a property that is true for every individual behavior.

What does that leave out? A lot more than you'd expect!

For one, we can't do "there *exists* a behavior where P is true". So we can't say that P is possible, even if we don't actually reach it. One example of this would be proving that a game is winnable. We call these [reachability properties](https://buttondown.com/hillelwayne/archive/proving-whats-possible). More advanced reachability properties would be things like "P is reachable from every initial state" or "P is reachable from any state where Q is true".

We also can't define properties over a *set* of behaviors. This is called a **hyperproperty**. Say we we're modeling phone hardware and want to verify that energy saving mode always uses less energy than normal mode. The property is "any sequence of actions uses no more power in energy saving mode than in regular mode." To refute this, you'd need to give me two behaviors that were identical *except* one starts in energy saving mode and the other does not, and the regular mode uses less power. One single behavior isn't enough to cut it, so this is impossible to naturally check in TLA+.

Hyperproperties might seem niche but they cover a [ton of security properties](https://en.wikipedia.org/wiki/Non-interference_\(security\)) and all statistical properties ("the 95%ile response time is 5ms").

Finally, and this one's a little more academic, we can't define properties over the state space as a whole. We can't say, for example, that there's only one path from state X to Y. I don't know how useful this would be in practice. Most of these kinds of "metaproperties" seem like they have potential to be meaningful, I just don't know what specifically.

### What "TLA+" "can" "do"

I kind of oversimplified when I said that TLA+ can't do these things. I mean that *if* you are writing a spec, and that spec directly corresponds to the system you want to build, then TLA+ can't express these as properties of your system. But you can [mimic](https://www.hillelwayne.com/post/software-mimicry/) two-step properties with [auxiliary variables](https://learntla.com/topics/aux-vars.html#), like storing all state changes in a `state_history` sequence and defining the property as an invariant on that sequence. You can mimic some hyperproperties with [self-composition](https://www.hillelwayne.com/post/hyperproperties/), where each behavior of the self-composed spec is two behaviors of the actual system. The main TLA+ model checker (TLC) can check the most basic reachability properties with the new [REACHABLE keyword](https://github.com/tlaplus/tlaplus/issues/860) and some state-space properties with [`TLCGet`](https://github.com/tlaplus/tlaplus/blob/master/tlatools/org.lamport.tlatools/src/tlc2/module/TLCGetSet.java). Andrew Helwer has a [crazy post about mimicking "always reachable"](https://ahelwer.ca/post/2026-09-26-reachability/) with fairness and "machine closure."

These are useful hacks, but they're still hacks. Each one takes a lot of cleverness to figure out and comes with serious drawbacks. Auxiliary variables ruin refinements, self-composition exponentiates your state space, etc. Hacks don't compose well with the other TLA+ features and don't cover all the intricacies of properties you'd want to express. And worst of all, they make your models look *weird* and *messy* and not correspond to your actual systems.

You could also use a different tool with a different focus. CTL can do reachability properties, PRISM probabilistic properties, etc. They trade off by being worse at things that TLA+ can do, and of course none of them handle properties we can't express logically.

Ultimately TLA+ is pretty good at picking a lot of low hanging fruit- invariants and liveness cover many things we care about, and TLA+ is reasonably good at expressing and checking them. There's a lot of potential (and a [a lot of pitfalls](https://buttondown.com/hillelwayne/archive/llms-are-bad-at-vibing-specifications/)) in using TLA+ to check vibe code. But there are also many things it cannot even express, let alone check.

* * *

## [Logic for Programmers Talk Online](https://www.youtube.com/watch?v=8Xrvyu5tV28)

Thanks to everybody who came to last week's livestream! You can watch the whole thing [here](https://www.youtube.com/watch?v=8Xrvyu5tV28).

[^1]: "Temporal logic of actions". It's a specification language used to find bugs in concurrent systems. [Here's an explanation of the name](https://buttondown.com/hillelwayne/archive/what-does-tla-mean-anyway/).

[^2]: (And as for the question of why a person so evangelical about formal methods [joined a testing company](https://antithesis.com/), well, there should be an article on their blog about that soon.)
