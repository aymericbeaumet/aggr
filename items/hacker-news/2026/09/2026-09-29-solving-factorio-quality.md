---
title: Solving Factorio Quality
link: https://exyr.org/2026/solving-factorio-quality/
source: hacker-news
published: 2026-09-29T02:27:54Z
updated: 2026-09-29T02:27:54Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- laurenth
content: extracted
html: 2026-09-29-solving-factorio-quality.html
preview:
  file: 2026-09-29-solving-factorio-quality.preview-db95095717c6.webp
  width: 256
  height: 161
  color: '#414141'
images:
- source: https://exyr.org/2026/solving-factorio-quality/wiki-modules.png
  original:
    file: 2026-09-29-solving-factorio-quality.image-4e93e6d51a59.png
    width: 790
    height: 496
  variants:
  - file: 2026-09-29-solving-factorio-quality.image-ba31451bdc4f.webp
    width: 320
    height: 201
  - file: 2026-09-29-solving-factorio-quality.image-d97be812178e.webp
    width: 640
    height: 402
  - file: 2026-09-29-solving-factorio-quality.image-74e377d965cb.webp
    width: 790
    height: 496
  color: '#373837'
- source: https://exyr.org/2026/solving-factorio-quality/wiki-probabilities.png
  original:
    file: 2026-09-29-solving-factorio-quality.image-65dc21df4cbf.png
    width: 964
    height: 438
  variants:
  - file: 2026-09-29-solving-factorio-quality.image-30ac256d8cfb.webp
    width: 320
    height: 145
  - file: 2026-09-29-solving-factorio-quality.image-7556d171b800.webp
    width: 640
    height: 291
  - file: 2026-09-29-solving-factorio-quality.image-95f1546ab0a9.webp
    width: 964
    height: 438
  color: '#444444'
- source: https://exyr.org/2026/solving-factorio-quality/wiki-24.8-percent.png
  original:
    file: 2026-09-29-solving-factorio-quality.image-9299ee972f39.png
    width: 878
    height: 624
  variants:
  - file: 2026-09-29-solving-factorio-quality.image-1d5c6f41e5f0.webp
    width: 320
    height: 227
  - file: 2026-09-29-solving-factorio-quality.image-f6d6ab094b4e.webp
    width: 640
    height: 455
  - file: 2026-09-29-solving-factorio-quality.image-4490c3810bdf.webp
    width: 878
    height: 624
  color: '#373737'
- source: https://exyr.org/2026/solving-factorio-quality/factoriolab.jpg
  original:
    file: 2026-09-29-solving-factorio-quality.image-6c7806b23573.jpg
    width: 1504
    height: 1384
  color: '#1e1e1e'
- source: https://exyr.org/2026/solving-factorio-quality/washing.jpg
  original:
    file: 2026-09-29-solving-factorio-quality.image-bb28f715c5ad.jpg
    width: 1200
    height: 947
  color: '#362825'
- source: https://exyr.org/2026/solving-factorio-quality/upcycling.jpg
  original:
    file: 2026-09-29-solving-factorio-quality.image-b87a68861e0b.jpg
    width: 1200
    height: 791
  color: '#463b15'
- source: https://exyr.org/2026/solving-factorio-quality/wiki-asteroid-reprocessing.png
  original:
    file: 2026-09-29-solving-factorio-quality.image-0e97f3945678.png
    width: 1256
    height: 456
  variants:
  - file: 2026-09-29-solving-factorio-quality.image-1154ad04ecc3.webp
    width: 320
    height: 116
  - file: 2026-09-29-solving-factorio-quality.image-6277609fa645.webp
    width: 640
    height: 232
  - file: 2026-09-29-solving-factorio-quality.image-a4067f637278.webp
    width: 960
    height: 349
  - file: 2026-09-29-solving-factorio-quality.image-5ae86adaed1a.webp
    width: 1256
    height: 456
  color: '#383838'
---

Simon Sapin, 2026-02-15

I play Factorio the normal way: by writing matrix math code to plan the factory.

But we’ll get to that. Or, *TL;DR*, go to my new [online calculator tool](https://factoqual.grebedoc.dev/).

### Intro to Factorio and Quality

[Factorio](https://factorio.com/) pretty much founded the factory video game genre: you play a character who harvests resources and combines them to craft increasingly complex items, which in turn enable more sophisticated crafting. So far this sounds a lot like Minecraft and many other survival games, but what sets factory games apart is the focus on automation: soon enough, most of the crafting is done not “by hand” by the character but by increasingly many machines, with various forms of logistics like conveyor belts to move items between machines or wherever they need to go. Some factory game go further and remove the player character altogether.

As “technologies” are unlocked in-game, Factorio offers many mechanisms to improve production. One of them is modules: crafting machines have a (limited) number of slots to accept different kinds of modules that affect their stats: speed modules make the machine run faster at the cost of more energy consumption, productivity modules increase yield from the same ingredients at the cost of speed and energy, etc.

Released in 2024, the Space Age extension adds new game mechanics including Quality: every item and recipe now come in five quality tiers: ⚀ normal, ⚁ uncommon, ⚂ rare, ⚃ epic, and ⚄ legendary. Depending on the item, each tier improves stats such as making crafting machines faster or making productivity modules more productive. High-quality items can be crafted directly from ingredients of the same quality, but the only way to *increase* quality is through the new quality modules.

![](https://exyr.org/2026/solving-factorio-quality/wiki-modules.png)\
 Quality modules can have quality too. Source: [Factorio wiki](https://wiki.factorio.com/Quality)

Modules affect the probability $Q$ of any quality increase. For a given craft, each quality tier increase after the first is another 10% chance. We can build a table of the probabilities of output quality depending on input quality:

![](https://exyr.org/2026/solving-factorio-quality/wiki-probabilities.png)\
 Source: [Factorio wiki](https://wiki.factorio.com/Quality)

For example, the maximum possible quality chance in a machine with four module slots is 24.8%:

![](https://exyr.org/2026/solving-factorio-quality/wiki-24.8-percent.png)\
 Jumping from normal to legendary in one step is only a 0.0248% chance. Source: [Factorio wiki](https://wiki.factorio.com/Quality)

Some players dislike the introduction of randomness to a game that was [mostly](https://wiki.factorio.com/Uranium_processing) deterministic, but [with enough repetitions probabilities become ratios](https://en.wikipedia.org/wiki/Law_of_large_numbers).

The probabilities are balanced so that even with multiple crafting steps (each a potential quality jumps), getting high-quality items unavoidably involves also crafting many unwanted low-quality ones.

To avoid the factory grinding to a halt when storage eventually gets full, Space Age also introduces the recycler: a new machine that destroys any item and (usually) returns 25% of its ingredients. This enables players to design “upcycling” contraptions that craft and recycle in a loop with quality modules until items reach the desired quality, at the cost of consuming many more ingredients:

 Source: [Factorio blog](https://www.factorio.com/blog/post/fff-375)

### Factory planning tools

Some video games are partly “played” outside of the game itself. [Blue Prince](https://www.blueprincegame.com/) fully expects its players to keep extensive notes of everything they see, but doesn’t provide an in-game notepad or similar tool. Factory games lend themselves to building [large spreadsheets](https://youtu.be/8PzhHwnX9ts?t=71) for resource accounting, but a select few players decide that spreadsheets are not powerful enough for factory planning and spent countless hours programming [dedicated tools](https://factoriolab.github.io/) that reproduce much of the game’s math to accurately model a production chain.

![](https://exyr.org/2026/solving-factorio-quality/factoriolab.jpg)\
 Example production chain in [Factoriolab](https://factoriolab.github.io)

This is all optional in factory games, it’s perfectly viable to play it by ear and just build more when seeing something lacking.

But I do like to plan in advance: how many machines of each kind do I need? How much yield can I expect? Where are the bottlenecks? The looping nature of quality upcycling makes this particularly challenging either to guess, or to calculate with existing tools.

### Matrix math

Let’s imagine:

- Some ingredients (for example iron plates) that come from an arbitrary production chain that may involve quality modules. Any given ingredient has a probability to be in each quality tier: ⚀ normal, ⚁ uncommon, ⚂ rare, ⚃ epic, and ⚄ legendary.
- Enough assembling machines with each recipe tier to craft all ingredients into some product (for example pipes). These machines have quality modules so that the a quality chance is 10%.

Let’s track the possible fates of one item:

A product of a given tier can come from ingredients of the same tier or lower. The total probability for this outcome is the sum of (independent) probabilites of different ways to get it. In turn, those are the product of the percentage chance of a specific quality jump times the probability of having the corresponding ingredient tier in the first place:

$$
\begin{align*}
p_⚀ &= i_⚀ ⋅ 90\% \\
p_⚁ &= i_⚀ ⋅ 9\%    &+ &i_⚁ ⋅ 90\% \\
p_⚂ &= i_⚀ ⋅ 0.9\%  &+ &i_⚁ ⋅ 9\%   &+ &i_⚂ ⋅ 90\% \\
p_⚃ &= i_⚀ ⋅ 0.09\% &+ &i_⚁ ⋅ 0.9\% &+ &i_⚂ ⋅ 9\% &+ &i_⚃ ⋅ 90\% \\
p_⚄ &= i_⚀ ⋅ 0.01\% &+ &i_⚁ ⋅ 0.1\% &+ &i_⚂ ⋅ 1\% &+ &i_⚃ ⋅ 10\% &+ i_⚄
\end{align*}
$$

(The percent sign can be thought of as implicit division by 100, so that “percentage of” is the same as multiplication.)

Here the percentage coefficients look [transposed](https://en.wikipedia.org/wiki/Transpose) across the diagonal compared to the quality jump probability table from the wiki, but that’s only because we’ve arranged product tiers vertically. Instead let’s group the probabilities of different tiers of the same item into row vectors:

$$
\begin{align*}
product    &= \begin{pmatrix*} p_⚀ & p_⚁ & p_⚂ & p_⚃ & p_⚄ \end{pmatrix*} \\
ingredient &= \begin{pmatrix*} i_⚀ & i_⚁ & i_⚂ & i_⚃ & i_⚄ \end{pmatrix*} \\
\end{align*}
$$

Now our [system of linear equations](https://en.wikipedia.org/wiki/System_of_linear_equations) can be written as a single equation where a vector is [multiplied](https://en.wikipedia.org/wiki/Matrix_multiplication) by a [transition matrix](https://en.wikipedia.org/wiki/State-transition_matrix) that matches the wiki’s table:

$$
\begin{align*}
{products} &= {ingredients} ⋅ T_{quality}(10\%) \\[1em]
T_{quality}(q) &=
\begin{pmatrix*}
  1-q & \frac{9q}{10} & \frac{9q}{100} & \frac{9q}{1000} & \frac{q}{1000} \\[0.3em]
  0   & 1-q           & \frac{9q}{10}  & \frac{9q}{100}  & \frac{q}{100}  \\[0.3em]
  0   & 0             & 1-q            & \frac{9q}{10}   & \frac{q}{10}   \\[0.3em]
  0   & 0             & 0              & 1-q             & q              \\[0.3em]
  0   & 0             & 0              & 0               & 1
\end{pmatrix*}
\end{align*}
$$

As an edge case, zero quality chance means no tier transformation. The corresponding transition matrix is the [identity matrix](https://en.wikipedia.org/wiki/Identity_matrix): $T_{quality}(0\%) = I_5$

This may not seem like much progress, but now a multi-step process can be computed through successive matrix multiplication. For example mining iron ore with 7.5% quality chance, then smelting it into iron plates with 5% quality chance, then crafting pipes with 10% chance. With no productivity bonus, we get these probabilities of end-products:

$$
\begin{align*}
pipe &= \begin{pmatrix*} 1 & 0 & 0 & 0 & 0 \end{pmatrix*} ⋅
    T_{quality}(7.5\%) ⋅
    T_{quality}(5\%) ⋅
    T_{quality}(10\%) \\
  &= \begin{pmatrix*} 0.925 & 0.0675 & 0.00675 & 0.000675 & 0.000075 \end{pmatrix*} ⋅
    T_{quality}(5\%) ⋅
    T_{quality}(10\%) \\
  &≈ \begin{pmatrix*} 0.87875 & 0.10575 & 0.013613 & 0.001665 & 0.000223 \end{pmatrix*} ⋅
    T_{quality}(10\%) \\
  &≈ \begin{pmatrix*} 0.790875 & 0.174263 & 0.029678 & 0.004467 & 0.000719 \end{pmatrix*}
\end{align*}
$$

### Quality strategies

While it is possible to use quality modules as much as possible and deal with [Every tier Everywhere All at Once](https://youtu.be/l5NA-5e0LbI?t=310), here we’ll focus on smaller self-contained systems.

#### “Gambling”: opportunistic quality without recycling

The easiest but also least effective is to craft from normal-quality ingredients, with quality modules, and not recycle anything. This can be done before unlocking the recycler but is only viable for a small number of items, such as crafting a few hundred asteroid collectors to hope to get a dozen uncommon or rare ones for an early space ship.

This is improved when the factory has another use for normal-quality items. For example if placing thousands of normal-quality solar panels on the ground, crafting them with quality modules gives a better yield of higher-quality ones for space ships before the output buffers fill up.

With a single step and no loop, this is simplest to calculate: the expected product tier distribution is the first row of the $T_{quality}(q)$ transition matrix or of the probability table found on the wiki.

#### “Washing”: pure recycling loop

For most items, the recycler reverses the main crafting recipe and returns 25% of the ingredients. But some items don’t have a crafting recipe (like ore) or it is considered irreversible (typically smelting and chemical processes). In that case the recycler produces either nothing or, 25% of the time, the same item. This process can improve quality if the recycler has quality modules. Repeating it in a loop, eventually all items will be either destroyed or improved until they reach any desired quality tier. Self-recycling items is arguably not the common case but let’s start here since the math is simpler.

![](https://exyr.org/2026/solving-factorio-quality/washing.jpg)\
 Example setup mining ore (with quality modules), and “washing” it until rare or above.

Let’s consider one item injected into the system, in this case from mining, and call $fresh_{unit}$ the 5-component row vector of probabilities of each quality tier.

After we transform that vector, the new probabilities may add up to less than one. The implicit remaining case is not having an item at all at a given place. For example, the recycler producing nothing 75% of the time can be represented by multiplying a vector of probabilities by $\frac{1}{4}$.

Filtering based on quality can also be represented with matrix multiplication. In the case of extracting rare or above:

$$
\begin{align*}
T_{filterKeep} &=
\begin{pmatrix*}
  1 & 0 & 0 & 0 & 0 \\
  0 & 1 & 0 & 0 & 0 \\
  0 & 0 & 0 & 0 & 0 \\
  0 & 0 & 0 & 0 & 0 \\
  0 & 0 & 0 & 0 & 0
\end{pmatrix*} \\
T_{filterExtract} &=
\begin{pmatrix*}
  0 & 0 & 0 & 0 & 0 \\
  0 & 0 & 0 & 0 & 0 \\
  0 & 0 & 1 & 0 & 0 \\
  0 & 0 & 0 & 1 & 0 \\
  0 & 0 & 0 & 0 & 1
\end{pmatrix*} = I_5 - T_{filterKeep}
\end{align*}
$$

The combined effect of one iteration through the loop is filtering to keep, then recycling with quality:

$$
\begin{align*}
L &= T_{filterKeep} · \frac{1}{4} · T_{quality}(q)
\end{align*}
$$

Now let’s consider the possible ways an item can be extracted:

- It originally had high enough quality to be extracted immediately: probability vector $fresh_{unit} · T_{filterExtract} = fresh_{unit} · L^0 · T_{filterExtract}$
- It went through the recycling loop exactly once: $fresh_{unit} · T_{filterKeep} · L · T_{filterExtract} = fresh_{unit} · L^1 · T_{filterExtract}$
- It went through recycling exactly twice: $fresh_{unit} · T_{filterKeep} · L · L · T_{filterExtract} = fresh_{unit} · L^2 · T_{filterExtract}$
- etc.

Any given item will eventually be extracted or destroyed, but there is no upper bound on how many loops that can take. The combination of all possible outcomes is an infinite sum:

$$
extracted_{unit} = fresh_{unit} · \bigg(\sum_{i=0}^{+∞} L^i\bigg) · T_{filterExtract}
$$

Infinite sums are rather inconvenient to calculate in finite time, although this one does converge to a finite value since the terms become exponentially smaller. We could compute until the terms because small enough for an approximation, but that would be unsatisfactory when an exact solution *is* possible.

##### Long-term average throughput

The key insight is that it doesn’t matter how many times a given item has already been through the loop, only what quality tier it has now. On a short time scale this will vary because of the random effect of quality modules but with enough repetitons [probabilities become ratios](https://en.wikipedia.org/wiki/Law_of_large_numbers).

So instead of probablities for a single item let’s consider the **average rate of items over a long enough period of time** going through a given part of the system, again as a 5-component vector for quality tiers. These vectors can be multiplied by the same transition matrices as before. In the case of a “washing” pure recycling loop, the relevant vectors are:

- $fresh$ items injected into the system, with any quality distribution, in this example based on quality modules in miners
- Items $fromRecycling$
- All items going through the $splitter$
- Items $extracted$ based on their quality tier, in this example rare or above
- Items $toRecycle$ with quality $q$, those not extracted

We treat $q$ and $fresh$ as a fixed parameters and other vectors as unknowns we want to resolve.

The system converges to a dynamic equilibrium where the following equations hold for long-term averages:

$$
\begin{align*}
splitter &= fresh + fromRecycling \\
splitter &= extracted + toRecycle \\
extracted &= splitter ⋅ T_{filterExtract} \\
toRecycle &= splitter ⋅ T_{filterKeep} \\
fromRecycling &= toRecycle ⋅ \frac{1}{4} T_{quality}(q)
\end{align*}
$$

We can rearrange and substitute:

$$
\begin{align*}
splitter = fresh + fromRecycling \\
splitter - fromRecycling = fresh \\
splitter - toRecycle ⋅ \frac{1}{4} T_{quality}(q) = fresh \\
splitter - splitter ⋅ T_{filterKeep} ⋅ \frac{1}{4} T_{quality}(q) = fresh \\
splitter ⋅ (I_5 - T_{filterKeep} ⋅ \frac{1}{4} T_{quality}(q)) = fresh \\
\end{align*}
$$

Transpose to get column vectors instead of row vectors and match the [classic convention](https://en.wikipedia.org/wiki/System_of_linear_equations#Matrix_equation):

$$
\begin{align*}
(splitter ⋅ (I_5 - T_{filterKeep} ⋅ \frac{1}{4} T_{quality}(q)))^T &= fresh^T \\
(I_5 - T_{filterKeep} ⋅ \frac{1}{4} T_{quality}(q))^T ⋅ splitter^T &= fresh^T
\end{align*}
$$

Introduce some new names:

$$
\begin{align*}
A &= (I_5 - T_{filterKeep} ⋅ \frac{1}{4} T_{quality}(q))^T \\
x &= splitter^T \\
b &= fresh^T
\end{align*}
$$

Now we have equilibrium as a system of linear equations in the classic $A·x = b$ form, that we can solve for $x$ using [Gaussian elimination](https://en.wikipedia.org/wiki/Gaussian_elimination). From $x$ we can easily compute $splitter$, then $toRecycle$ (for the number of recyclers needed) and $extracted$ (for the overall yield of the system).

If the example setup above was scaled to mine 100 ore per second, the parameters would be:

$$
\begin{align*}
fresh &= \begin{pmatrix*} 90 & 9 & 0.9 & 0.09 & 0.01 \end{pmatrix*} \\
T_{filterExtract} &= filter(≥ rare) \\
q &= 10\%
\end{align*}
$$

And the solution with Gaussian Elimination:

$$
\begin{align*}
toRecycle &≈ \begin{pmatrix*} 116.1291 & 14.9844 & 0 & 0 & 0 \end{pmatrix*} \\
extracted &≈ \begin{pmatrix*} 0 & 0 & 1.4985 & 0.1499 & 0.0167 \end{pmatrix*} \\
\end{align*}
$$

Converting from per second, the extracted rates are close to 90 per minute rare, 9 per minute epic, and 1 per minute legendary.

* * *

For another example:

- Injecting only normal-quality fresh items, for now in some arbitrary unit: $fresh = \begin{pmatrix*} 1 & 0 & 0 & 0 & 0 \end{pmatrix*}$
- Recycling with the best available quality modules: $q = 24.8\%$
- Extracting only legendary-quality items

Solving the equation gives:

$$
\begin{align*}
toRecycle &≈ \begin{pmatrix*} 1.231528 & 0.08463 & 0.014279 & 0.00241 & 0 \end{pmatrix*} \\
extracted &≈ \begin{pmatrix*} 0 & 0 & 0 & 0 & 0.000366716 \end{pmatrix*} \\
\end{align*}
$$

So for items that recycle to themselves, “brute-force” washing consumes on average $\frac{1}{extracted_⚄} ≈ 2726.91$ normal-quality inputs for every legendary output. This matches the ratio that [others](https://exyr.org/2026/solving-factorio-quality/#ack) have calculated. The recycling capacity required is just shy of $\frac{4}{3}$ of the fresh input rate.

#### “Upcycling”: crafting + recycling loop

For items where recycling *does* return ingredients, we can chain crafting machines with recyclers to form a loop:

![](https://exyr.org/2026/solving-factorio-quality/upcycling.jpg)\
 Example setup upcycling construction robots until lengendary.

Most crafting recipes require multiple ingredients in various quantities, but here we’ll abstract over this and consider “the set of ingredients for one craft” as the base unit for our measurements. In this example, each machine crafting construction robots can consume 4 electronic circuits per second and 2 flying robot frames per second, but we’ll call that “2 ingredients per second”.

So we’re measuring long-term avarage rates of ingredients and products, each in five quality tiers, but they only appear in separate parts of the system so we’ll stick with 5-components vectors and don’t need to move to 10-dimensional math. (Wink wink foreshadowing)

The example setup has fresh normal-quality ingredients brought by robots into the blue requester chest, and extracts legendary products. In the general case, we could imagine ingredients or products of any quality distribution being produced elsewhere and injected into the system. Similarly, we could decide to extract products *or ingredients* (or both) of any quality tier. (Sometimes an ingredient may be more useful than a product to have in high quality, but upcycling that specific product may have better yield than other methods.)

So the 5-component row vectors we’ll consider are, for ingredients:

- $freshI$ injected into the system, with any quality distribution
- Ingredients $fromRecycling$
- $totalI$
- $extractedI$ based on their quality tier, in this example none
- Ingredients $toCraft$ new products from

And for products:

- $freshP$ injected into the system, with any quality distribution
- Products $fromCrafting$
- $totalP$
- $extractedP$ based on their quality tier, in this example lengendary
- Products $toRecycle$

We treat $freshI$ and $freshP$ as fixed parameters, and other vectors as unknown we want to resolve.

Again the transition matrices for filtering based on quality tier are complementary parts of the identity matrix. In this example:

$$
\begin{align*}
T^{ingredients}_{filterKeep} &= I_5 \\
T^{ingredients}_{filterExtract} &= 0_5 \\
T^{products}_{filterKeep} &=
\begin{pmatrix*}
  1 & 0 & 0 & 0 & 0 \\
  0 & 1 & 0 & 0 & 0 \\
  0 & 0 & 1 & 0 & 0 \\
  0 & 0 & 0 & 1 & 0 \\
  0 & 0 & 0 & 0 & 0
\end{pmatrix*} \\
T^{products}_{filterExtract} &=
\begin{pmatrix*}
  0 & 0 & 0 & 0 & 0 \\
  0 & 0 & 0 & 0 & 0 \\
  0 & 0 & 0 & 0 & 0 \\
  0 & 0 & 0 & 0 & 0 \\
  0 & 0 & 0 & 0 & 1
\end{pmatrix*} = I_5 - T^{products}_{filterKeep}
\end{align*}
$$

We account for a crafting $productivityBonus$ which is 0% in this example but could be greater with some crafting machines or with productivity modules. The quality chance may also be different for crafting v.s. recycling (for example when crafting with productivity modules, or based on the machine’s number of modules slots).

For short(er), let’s call:

$$
\begin{align*}
T_{craft} &= (1 + productivityBonus) ⋅ T_{quality}(q_{crafting}) \\
T_{recycle} &= \frac{1}{4} T_{quality}(q_{recycling})
\end{align*}
$$

The system converges to equilibrium where:

$$
\begin{align*}
totalI &= freshI + fromRecycling \\
totalI &= extractedI + toCraft \\
extractedI &= totalI ⋅ T^{ingredients}_{filterExtract} \\
toCraft &= totalI ⋅ T^{ingredients}_{filterKeep} \\
fromCrafting &= toCraft ⋅ T_{craft} \\[2em]
totalP &= freshP + fromCrafting \\
totalP &= extractedP + toRecycle \\
extractedP &= totalP ⋅ T^{products}_{filterExtract} \\
toRecycle &= totalP ⋅ T^{products}_{filterKeep} \\
fromRecycling &= toRecycle ⋅ T_{recycle}
\end{align*}
$$

We can rearrange and substitute:

$$
\begin{align*}
totalP &= freshP + fromCrafting \\
totalP &= freshP + toCraft ⋅ T_{craft} \\
totalP &= freshP + totalI ⋅ T^{ingredients}_{filterKeep} ⋅ T_{craft} \\
totalP &= freshP + (freshI + fromRecycling) ⋅ T^{ingredients}_{filterKeep} ⋅ T_{craft} \\
totalP &= freshP + (freshI + toRecycle ⋅ T_{recycle}) ⋅ T^{ingredients}_{filterKeep} ⋅ T_{craft} \\
totalP &= freshP + (freshI + totalP ⋅ T^{products}_{filterKeep} ⋅ T_{recycle}) ⋅ T^{ingredients}_{filterKeep} ⋅ T_{craft}
\end{align*}
$$

Let’s introduce a couple more names:

$$
\begin{align*}
\begin{align*}
T_{filterRecycle} &= T^{products}_{filterKeep} ⋅ T_{recycle} \\
T_{filterCraft} &= T^{ingredients}_{filterKeep} ⋅ T_{craft} \\[1em]
totalP &= freshP + (freshI + totalP ⋅ T_{filterRecycle}) ⋅ T_{filterCraft} \\
\end{align*}\\
\begin{align*}
totalP - totalP ⋅ T_{filterRecycle} ⋅ T_{filterCraft} &= freshP + freshI ⋅ T_{filterCraft} \\
totalP ⋅ (I_5 - T_{filterRecycle} ⋅ T_{filterCraft}) &= freshP + freshI ⋅ T_{filterCraft} \\
(I_5 - T_{filterRecycle} ⋅ T_{filterCraft})^T ⋅ totalP^T &= (freshP + freshI ⋅ T_{filterCraft})^T
\end{align*}
\end{align*}
$$

We’ve again massaged our problem into the standard $A·x = b$ form with:

$$
\begin{align*}
A &= (I_5 - T_{filterRecycle} ⋅ T_{filterCraft})^T \\
x &= totalP^T \\
b &= (freshP + freshI ⋅ T_{filterCraft})^T
\end{align*}
$$

Solving for $x$ gives us $totalP$, and plugging that into the original equilibrium equations gives everything else.

In the example setup above, scaled for now to an arbitrary unit, the parameters are:

$$
\begin{align*}
freshI_{unit} &= \begin{pmatrix*} 1 & 0 & 0 & 0 & 0 \end{pmatrix*} \\
freshP &= \begin{pmatrix*} 0 & 0 & 0 & 0 & 0 \end{pmatrix*} \\
T^{ingredients}_{filterExtract} &= filter(nothing) \\
T^{products}_{filterExtract} &= filter(legendary) \\
productivityBonus &= 0\% \\
q_{crafting} &= 24.8\% \\
q_{recycling} &= 24.8\%
\end{align*}
$$

And the solution with Gaussian Elimination:

$$
\begin{align*}
toCraft_{unit} &≈ \begin{pmatrix*} 1.164655 & 0.113836 & 0.039404 & 0.011133 & 0.002154 \end{pmatrix*} \\
toRecycle_{unit} &≈ \begin{pmatrix*} 0.87582 & 0.345555 & 0.081035 & 0.022307 & 0 \end{pmatrix*} \\
extractedI_{unit} &= \begin{pmatrix*} 0 & 0 & 0 & 0 & 0 \end{pmatrix*} \\
extractedP_{unit} &≈ \begin{pmatrix*} 0 & 0 & 0 & 0 & 0.006464 \end{pmatrix*}
\end{align*}
$$

In practice, the bottleneck of this system is the crafting speed of normal-quality products: 360 per minute. So let’s multiply everything by $\frac{360}{toCraft_{unit_⚀}}$

$$
\begin{align*}
freshI_{scaled} &≈ \begin{pmatrix*} 309.10 & 0 & 0 & 0 & 0 \end{pmatrix*} \\
toCraft_{scaled} &≈ \begin{pmatrix*} 360 & 35.18 & 12.17 & 3.44 & 0.66 \end{pmatrix*} \\
toRecycle_{scaled} &≈ \begin{pmatrix*} 270.72 & 106.81 & 25.04 & 6.89 & 0 \end{pmatrix*} \\
extractedP_{scaled} &≈ \begin{pmatrix*} 0 & 0 & 0 & 0 & 1.99 \end{pmatrix*}
\end{align*}
$$

In conclusion, this system produces on average almost 2 legendary construction robots per minute, and consumes about 309 sets of ingredients (309 frames + 618 circuits) per minute.

* * *

For a select few items in Space Age, the productivity bonus can be increased through repeatable research. The game enforces a hard cap of +300% bonus so that crafting then recycling returns at most the ingredients that we started with, never more.

Productivity research levels get exponentially expensive so let’s assume that we use also productivity modules to reach the cap, meaning we don’t have quality modules in crafting machines. And this time, let’s inject fresh products instead of ingredients.

$$
\begin{align*}
freshI &= \begin{pmatrix*} 0 & 0 & 0 & 0 & 0 \end{pmatrix*} \\
freshP_{unit} &= \begin{pmatrix*} 1 & 0 & 0 & 0 & 0 \end{pmatrix*} \\
T^{ingredients}_{filterExtract} &= filter(nothing) \\
T^{products}_{filterExtract} &= filter(legendary) \\
productivityBonus &= +300\% \\
q_{crafting} &= 0\% \\
q_{recycling} &= 24.8\%
\end{align*}
$$

Our model predicts:

$$
\begin{align*}
toCraft_{unit} &≈ \begin{pmatrix*} 0.758 & 0.907 & 0.907 & 0.907 & 0.25 \end{pmatrix*} \\
toRecycle_{unit} &≈ \begin{pmatrix*} 4.032 & 3.629 & 3.629 & 3.629 & 0 \end{pmatrix*} \\
extractedI_{unit} &= \begin{pmatrix*} 0 & 0 & 0 & 0 & 0 \end{pmatrix*} \\
extractedP_{unit} &= \begin{pmatrix*} 0 & 0 & 0 & 0 & 1 \end{pmatrix*}
\end{align*}
$$

Perhaps surprisingly, the rates for intermediate quality tiers are identical.

A maximal productivity bonus enables turning a normal quality product into a legendary one without any ressource loss, at the cost of many machines and modules to get significant throughput.

#### [“Space casino”](https://youtu.be/jHrY1RtWHPw?t=1078): asteroid reprocessing loop

In Space Age, each space platform is a mini-factory. Instead of mining ore from the ground they collect asteroid chunks and crush them to get resources. Those can be used for a space ship’s own fuel and ammunition, or to feed a production chain in space whose end-product is sent to a planet’s ground.

Three different types of chunks (metallic, carbonic, and oxide) yield different ressources and are more or less frequent in different regions of the solar system. Asteroid chunks can be “reprocessed” for a chance to get a different type (or nothing).

![](https://exyr.org/2026/solving-factorio-quality/wiki-asteroid-reprocessing.png)\
 Asteroid reprocessing recipes. Source: [Factorio wiki](https://wiki.factorio.com/Crusher)

Assuming enough crushers with each recipe, we can represent this process mathematically as multiplying a row vector by a square 3×3 transition matrix:

$$
\begin{align*}
fromReprocessing &= toReprocess ⋅ T_{reprocessing} \\[1em]
T_{reprocesing} &=
\begin{pmatrix*}
  40\% & 20\% & 20\% \\
  20\% & 40\% & 20\% \\
  20\% & 20\% & 40\%
\end{pmatrix*}
\end{align*}
$$

Reprocessing also accepts quality modules so it can be used in a loop much like “washing”. Crushing legendary chunks yields legendary versions of some base resources that can be used for crafting other legendary items.

This time we’ll have conveyor belts transporting asteroid chunks of any of three types, each in any of five quality tiers, for a total of 15 possible items. We’ll represent this with 15-component row vectors and 15×15 transition matrices. We build up the latter as [block matrices](https://en.wikipedia.org/wiki/Block_matrix) made of nine 5×5 blocks:

$$
\begin{align*}
T^{15}_{reprocessing} &=
\begin{pmatrix*}
  40\% ⋅ I_5 & 20\% ⋅ I_5 & 20\% ⋅ I_5 \\
  20\% ⋅ I_5 & 40\% ⋅ I_5 & 20\% ⋅ I_5 \\
  20\% ⋅ I_5 & 20\% ⋅ I_5 & 40\% ⋅ I_5
\end{pmatrix*} \\[2em]
T^{15}_{quality}(q) &=
\begin{pmatrix*}
  T_{quality}(q) & 0 & 0 \\
  0 & T_{quality}(q) & 0 \\
  0 & 0 & T_{quality}(q)
\end{pmatrix*} \\[2em]
T^{15}_{filterKeep} &=
\begin{pmatrix*}
  T_{filterKeepMetallic} & 0 & 0 \\
  0 & T_{filterKeepCarbonic} & 0 \\
  0 & 0 & T_{filterKeepOxide}
\end{pmatrix*} \\[2em]
total &= fresh + fromReprocessing \\
fromReprocessing &= toReprocess ⋅ T^{15}_{reprocessing} ⋅ T^{15}_{quality}(q) \\
toReprocess &= total ⋅ T^{15}_{filterKeep} \\
extracted &= total ⋅ T^{15}_{filterExtract} = total ⋅ (I_{15} - T^{15}_{filterKeep})
\end{align*}
$$

Aside from the higher dimension, the math is the same as for washing and we end up with a system of linear equations $A·x = b$ to solve for $x$, with:

$$
\begin{align*}
A &= (I_{15} - T^{15}_{filterKeep} ⋅ T^{15}_{reprocessing} ⋅ T^{15}_{quality}(q))^T \\
x &= total^T \\
b &= fresh^T \\
\end{align*}
$$

Asteroid crushers have two module slots, so the best possible quality chance for reprocessing is $q = 12.4\%$. Asteroid collectors don’t have any module slot, so freshly-collected chunks are always normal-quality. We build the 15-component $fresh$ vector from its 3 components for normal-quality rate of each asteroid type (metallic, carbonic, oxide). For example in Nauvis orbit: $fresh_⚀ = \begin{pmatrix*} \frac{3}{6} & \frac{2}{6} & \frac{1}{6} \end{pmatrix*}$

Let’s say that we only extract legendary oxide chunks and reprocess everything else. Solving the equation gives two complementary 15-components row vectors $toReprocess$ and $extracted$ that we can rearrange into 3×5 tables

Example: in Nauvis orbit, extract legendary oxide chunks and reprocess everything else:

$$
\begin{align*}
fresh_⚀ &= \begin{pmatrix*} \frac{3}{6} & \frac{2}{6} & \frac{1}{6} \end{pmatrix*} \\
q &= 12.4\%
\end{align*}
$$

| Extracted |         |         |         |         |
| --------- | ------- | ------- | ------- | ------- |
| 1.31616   | 0.33791 | 0.13314 | 0.05286 | 0.0175  |
| 1.11409   | 0.33244 | 0.13245 | 0.05277 | 0.01748 |
| 0.91202   | 0.32697 | 0.13175 | 0.05268 | 0       |
| 0         | 0       | 0       | 0       | 0       |
| 0         | 0       | 0       | 0       | 0       |
| 0         | 0       | 0       | 0       | 0.01398 |

We observe:

- As the quality tier increases, the distribution of asteroid types quickly converges to an even $\frac{1}{3}$ each. The distribution of normal-quality fresh input has negligible impact on that of legendary-quality chunks going through the system.
- When extracting a single type of legendary asteroids, the legendary yield is about $\frac{1}{71.5} ≈ 1.4\%$ of the total fresh input. This is much better than about $\frac{1}{2726}$ for “washing” a.k.a. pure recycling, thanks to reprocessing destroying its input only 20% of the time v.s. 75% for recycling.

#### Other strategies

There are other ways to get quality items that don’t neatly fit in the categories above (honorable mention to [“The LDS Shuffle”](https://www.youtube.com/watch?v=MO519YUYn0k)) and I’m sure folks will come up with more. But if a loop makes it tricky to calculate their behavior we can use the same ideas:

- Represent throughput of multiple kinds of items as vectors
- Represent linear transformations (crafting, recycling, …) as matrix multiplication
- Represent dynamic equilibrium as a matrix equation
- Solve the equation, using Gaussian Elimination if needed

### Making an interactive calculator tool

Doing matrix math by hand is obviously tedious and error-prone, let’s have computers do it for us.

I started with Rust out of habit. [`nalgebra`](https://nalgebra.rs/) works out great for the matrix math we do here. In includes multiple solvers for $A·x = b$ systems of linear equations, but those pretty much require the scalar type for matrix and vector components to be a floating point number `f32` or `f64`, whereas the base library is more generic.

In practice floating point would be perfectly adequate, but it is a fixed-precision approximation so every step of computation potentially introduces some error. Wouldn’t it be nice to get an exact result, just because we can? The matrix sizes and number of operations are fixed and relatively small, so we don’t need to optimize the code for speed.

Every operation we use is ultimately addition, substraction, multiplication, or division. So if all of our parameters are [rational](https://en.wikipedia.org/wiki/Rational_number) the result will be too. [`num_rational`](https://crates.io/crates/num_rational) represents rationals extactly as a pair of (generic) integers. I could reach for some infinite-precision `BigInt` library if needed, but the built-in `i128` with [overflow checks](https://doc.rust-lang.org/cargo/reference/profiles.html#overflow-checks) turns out to be sufficient for 15×15 martices. (`i64` is sufficient for 5×5.)

At this point I had a functional Rust library doing all of the math above, but editing source code to tweak parameters isn’t a nice user experience. I’d much prefer something like [Factoriolab](https://factoriolab.github.io/). And to be easy to use by other people it really should be on the web.

My Rust code can be compiled to WebAssembly but then I would need to either:

- Build the entire GUI with Rust + wasm as well. It’s possible but the tooling [isn’t great yet](https://fasterthanli.me/articles/does-dioxus-spark-joy)
- Build the GUI in JavaScript or TypeScript (taking advantage of mature tools) and bridge into wasm for the math. But because of the many parameters the API surface is significant, and doing that much bridging isn’t fun

So I ended up rewriting the whole thing in TypeScript. [`BigInt`](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/BigInt) is built-in, Factoriolab already has a good open-source [`rational` library](https://github.com/factoriolab/factoriolab/blob/e06e27bef0ab1f317ab9e19f9c5520cf7ff7dc4d/src/rational/rational.ts), and making a generic matrix library isn’t too hard.

As to building an interactive GUI in the browser, last time I did much of it jQuery was the hot new thing. I didn’t feel like learning React so I settled on:

- [VanJS](https://vanjs.org/) for minimal reactive goodness
- [Vite](https://vite.dev/) for TypeScript wrangling and a reload-on-save dev server
- [Grebedoc](https://grebedoc.dev/) for static file hosting

All together, [factoqual.grebedoc.dev](https://factoqual.grebedoc.dev/) now provides an interactive GUI for planning Quality upcyclers. Its source code is published at [codeberg.org/SimonSapin/factoqual](https://codeberg.org/SimonSapin/factoqual).

### Acknowledgments

I was heavily inspired by Daniel Monteiro’s [blog posts](https://dfamonteiro.com/tags/factorio-quality/) on quality math. Others have done similar work, including [Konage](https://forums.factorio.com/viewtopic.php?t=124772) and [Factorio wiki contributors](https://wiki.factorio.com/Tutorial:Quality_upcycling_math). But I believe the exact solution from solving equilibrium equation as shown here is new, as opposed to iterative approximation an infinite sum.
