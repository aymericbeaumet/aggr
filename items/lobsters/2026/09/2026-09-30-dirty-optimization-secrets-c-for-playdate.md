---
title: Dirty Optimization Secrets (C for Playdate)
link: https://devforum.play.date/t/dirty-optimization-secrets-c-for-playdate/23011
source: lobsters
published: 2026-09-30T05:05:56Z
updated: 2026-09-30T05:05:56Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- devforum.play.date via strugee
labels:
- hardware
summary: Comments
content: extracted
html: 2026-09-30-dirty-optimization-secrets-c-for-playdate.html
preview:
  file: 2026-09-30-dirty-optimization-secrets-c-for-playdate.preview-71784e68e974.webp
  width: 256
  height: 134
  color: '#d1a836'
images:
- source: https://devforum-cdn.play.date/original/2X/c/c0b050e7998ebafd5d1721a985b313050be81ce1.png
  original:
    file: 2026-09-30-dirty-optimization-secrets-c-for-playdate.image-c0b050e7998e.png
    width: 1200
    height: 630
  color: '#fecb40'
---

These are some advanced optimization tricks I've discovered while [@stonerl](https://devforum.play.date/u/stonerl) and I have been cooking the [full-speed gameboy emulator](https://devforum.play.date/t/60-fps-gameboy-emulation/22865). These are not the typical tips for optimizing games you can find elsewhere, so if you are not familiar with existing techniques then these may not be as useful for you.

These kinds of optimization techniques are most likely to be helpful if you're trying to get very performant code for something like an emulator, a large simulation (such as a factorio-like), a 3D renderer, a codec, and so on.

## 1\. Fast CPU, slow Cache

This comes from research [@StiNKz](https://devforum.play.date/u/stinkz) did, and you can find their results on discord [here](https://discord.com/channels/675983554655551509/1176287444732231871/1329362403141353473). Your model of the playdate's CPU should be that it is *very fast CPU* but is slow to access memory, especially memory not in the cache. This means if your code is operating primarily on registers and not loading lots of data, it will likely go blazingly fast, even if it has lots of loops and branch mis-prediction.

## 2\. Instruction Cache

`-O3` is not necessarily fastest; `-Os` might be much better. Playdate has a small instruction cache (Rev A's is **only 4 kilobytes**), so if your **core code** -- say, your emulator interpreter loop, your fragment renderer, your physics engine -- does not fit within 4 kilobytes (apparently 16 kb for Rev B?) then it will likely not be as performant as code that's more compressed (even if more compressed code requires many more CPU operations.) The first thing I did to improve performance for gameboy emulation was to squeeze the 20 kb emulator core into just 2 kb by removing a giant switch table and expressing it using much less code with just a few branches. (And using `-Os`.) The behaviour was identical, but it was much faster as a result.

Now, that's just your *core* code that needs to fit in this 4 kb cache. You can take *rare operations* (e.g. opcodes that almost never occur, or edge cases that are very unlikely in your simulation) and put those elsewhere.

Ideally, your 4 kb core code should all be placed at contiguous addresses by the linker. How can you achieve this? Use `__attribute__((section(".text.<your-section-name-here>")))` on each *core function* to ensure the linker places them in one contiguous section. You can do slightly more advanced tricks with a linker map (see next section).

## 3\. Custom Linker Map

Copy `link_map.ld` from the `C_API/buildsupport` folder into your project, then add this line to your makefile before you include `common.mk`:

```auto
override LDSCRIPT=./link_map.ld
```

This lets you use a custom linker script portably. If you haven't edited a linker script before (as most people haven't), don't worry, there's not much to it. You can specify that all the code from certain files be put in a certain spots like so (assuming your .o files are placed in `build/`):

```auto

/* custom code section (i.e. with __attribute__((section(".text.blah"))) */
*(.text.blah)

/* everything from foo.c */
build/foo.o(.text)
build/foo.o(.text.*)

/* Align to 32 bytes */
. = ALIGN(32);

/* the code not explicity mentioned above */
*(.text)
*(.text.*)
```

[Example linker script](https://github.com/CrankBoyHQ/crankboy-app/blob/0c80fae6148e9b77942f25aa9e6962f68c8928c7/link_map.ld).

## 4\. Inspect symbols

Keep this command handy:

```auto
nm Source/pdex.elf | sort > syms.txt
```

This produces a list of all symbols (functions, variables, etc.) and their associated addresses. You can check how big your core code is quite easily and see whether or not it fits in icache.

Example output (portion):

```auto
00019361 t write_cart_ram_file /* <-- this is a function*/
00019420 t $d
00019440 t $t
00019441 t gb_error
000194dc t $d
00019500 t $t
00019501 t PGB_GameScene_menu
00019638 t $d
0001965c t $t
00019a04 t $d
00019a30 t $t
00019a31 T PGB_LibraryConfirmModal
00019a44 t $d
00019a50 t $t
00019a51 t gb_save_to_disk.isra.0
00019aa8 t $d
00019ac0 t $t
00019ac1 T __gb_write_vram
00019b10 t $t
00019b11 t PGB_GameScene_free
00019b9c t $d
00019bc0 t $t
00019bc1 T PGB_GameScene_new
```

## 5\. DTCM acceleration

There is a small region of memory called *tightly-coupled memory* (TCM) which is faster than normal memory. The stack sits in the TCM area, so if you have a stack-allocated object it will generally be faster than something on the heap or a static variable.

Indeed, if you're going to operate on a certain struct for a while, it can be worth `memcpy`-ing it onto the stack, operating on it there, and then `memcpy`-ing it back. [@RPDev](https://devforum.play.date/u/rpdev) implemented this for PlayGB and got a big perf boost.

```c
// slow --
long_operation(&global_var);

// faster --
some_type copy;
memcpy(&copy, &global_var, sizeof(global_var));
long_operation(&copy);
memcpy(&global_var, &copy, sizeof(global_var));
```

But we can do even better! Why not let the variable live on the stack permanently instead of on the heap? Then we can skip two `memcpy`s. Well there's a reason we can't do this -- we don't have control over `main`, so we can't keep anything permanently on the stack. There's a workaround though -- let's store it on the other end of the stack (that is, the low address, the part we should never reach lest we risk a stack overflow.) You can use `__builtin_frame_address(0)` in your `eventHandler` on `kEventInit` in order to determine the high-address end of the stack, then subtract something less than 10 kb (I found `0x2180` has been safe) in order to reach the low-end of the stack. If [this feature](https://devforum.play.date/t/api-function-get-address-of-stack-top-and-bottom/22094) is eventually added, that is a better way to find the low-address region of the stack.

If you store something in this low region, which I'll call the `dtcm_mempool`, it should persist from update to update. Place a canary at the start and end of the `dtcm_mempool` and check them at least once per update to make sure that a stack overflow hasn't corrupted the `dtcm_mempool`.

By the way, there are other parts of memory that are also in the TCM region, such as the playdate's framebuffer. You can put data there for efficient access if you don't have enough stack space (though obviously you'll need to deal with the fact that this data will actually appear on-screen if you render it!)

## 6\. ITCM acceleration

Data isn't the only thing that we can put on the stack (or in the `dtcm_mempool`) -- we can put *code* there too. It's unclear whether this results in any acceleration for Rev B, though more research is needed. But on Rev A it's the secret sauce for efficient gameboy emulation.

Now, the Playdate won't automatically put your code into the TCM region, so you'll need to copy it there from `.text` manually. Define this macro in C to mark functions as belonging to the itcm region:

```auto
#define _itcm __attribute__((section(".itcm"))) __attribute__((short_call))
```

You can place `_itcm` before a function definition to mark it as belonging to `itcm`.

Next, in the linker map, put this:

```c
__itcm_start = .;
*(.itcm)
__itcm_end = .
```

This causes two symbols to be exposed to C, one at the start of your `.itcm` code and one at the end. Using this, you can `memcpy` the entire `.itcm` code to wherever you like (including into TCM). **Important**: make sure that the address you copy to is **congruent to the original address** mod 2, since the Cortex M7 uses the lowest bit of a function pointer to indicate whether the code is Thumb or ARM. Also important: for this same reason, **you can't `memcpy` from a function address directly**, since it the function address is actually going to be 1 byte *after* the start of the function. ARM is weird.

```c
extern char __itcm_start, __itcm_end;

char dst_itcm_region[(uintptr_t)__itcm_end - (uintptr_t)__itcm_start];
memcpy(&dst_itcm_region, &__itcm_start, &__itcm_end - &__itcm_start);

// call the ITCM copy of the function
((fn_ptr_t)((void*)some_itcm_fn - __itcm_start + dst_itcm_region))(args...);
```

Considerations:

- DON'T enable -fPIC, since rather counterintuitively the relocation table breaks work when the code is relocated like this.
- use the `shortcall` attribute for `.itcm` code, as in the macro above, since when one itcm function calls another itcm function it needs to do a *relative* jump (otherwise it could jump to its original non-relocated location).
- Any function outside of the .itcm region which your .itcm code can invoke must be marked with `__attribute__((longcall))`, which forces the code to do an *absolute* jump (otherwise it will do a relative jump into somewhere unknown.)
- flush the icache after modifying/copying around code. There's a function for this in the playdate API.

**This will likely crash when you first try it if you don't get it exactly right**. Therefore, start small with this -- try just copying a single function which does nothing except returns a number, for instance, and work your way up from there. Be very careful when you port a large function over to itcm, since it might call other functions by shortcall (and so it will most surely crash).

## 7\. Performance Lottery

As you modify your code, you'll notice without apparent pattern that some builds are slower and some builds are faster, sometimes significantly so (like 50% faster.) There are many causes for this, but there are two causes in particular that I have identified as especially prominent: cache misalignment and branch prediction tables.

### **Cache Misalignment**

The cache line size is 32 bytes, meaning that your application loads 32 bytes of instructions at a time. If there are two functions of size 16 bytes each, and they're right next to each other, *and the first ones address is divisible by 32 exactly*, then loading one of those functions will cause the other function to enter the cache as well. On the flip side, if one function is 32 bytes long, but its starting address *isn't divisible by 32*, then it will straddle the boundary of two cache lines (and that's bad).

How does this cause a performance lottery effect? Well, it means adding one function whose size is not divisible by 32 somewhere early in the code causes every other function to shift down, and their addresses might no longer be congruent to their old addresses mod 32. At this point it's a toss-up whether it's faster or slower.

How to minimize this effect? Every now and then in your code, force something to be aligned to 32-bytes. You can do this in C with `__attribute__((aligned(32)))`, or from your linker map with `. = ALIGN(32);`. The advantage of doing this from your linker map is that you can specify an offset from 32 bytes manually, e.g. `. = ALIGN(32); . += 0xC`. You should look at the output of `nm` (section 4) from your *most recent fast build* (the last build which did well on the performance lottery) to figure out what manual offset to use here. I like to put `. = ALIGN(32)` before each source file and section in my linker map to ensure that they are all quarantined from each other in terms of cache line.

By the way, you can use the compiler option `-falign-loops=32` to ensure that loops get aligned to 32-bytes in code, which could also help.

### **Branch prediction tables**

The details of how branch prediction works on the Cortex M7 CPU are not public knowledge. By my estimate -- and I could be completely wrong about this! -- there is a table of prediction values for 1024 unique instruction addresses. In layman's terms, this means that if two `if` statements are a multiple of 1024 bytes apart from each other, then the branch predictor will treat them the same, causing lots of mispredictions. Therefore, in your linker map, sprinkle in a handful of `. = ALIGN(1024); . += n;` for varying values of *n* in the range 0-1024. (Better yet, choose *n* based on the address of the first symbol following it in the last good build.) Now, that is 4+ kilobytes of wasted space, so don't do many of these.

I might be wrong about branch prediction being what's causing this second-order performance lottery, but regardless, ALIGN(1024) trick has helped, beyond just the fact that 1024 is divisible by 32.

I found the performance lottery has almost disappeared since implementing these tricks.

## 8\. Prefetching

I haven't observed any improvement in performance using prefetching, but I might just not be doing it smartly enough. [@FReDs72](https://devforum.play.date/u/freds72) describes a case where prefetching helped in this [discord thread](https://discord.com/channels/675983554655551509/1176287444732231871/1357778126813397142). Performance improvements are likely quite fine-grained when it comes to prefetching, so perhaps measure your frame time with the high-precision timer instead of the normal FPS gauge.

In theory, prefetching should help in cases where you have indirection. For instance, if iterating through a list of pointers (`int** list`), you might benefit from (a) iterating through the whole list in advance and prefetching each `list[n]`, or (b) prefetching `list[i+1]` before visiting `list[i]`.
