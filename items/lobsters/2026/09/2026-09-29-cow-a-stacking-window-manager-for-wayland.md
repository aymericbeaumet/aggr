---
title: CoW — a stacking window manager for Wayland
link: https://cow-wm.codeberg.page/cow/
source: lobsters
published: 2026-09-29T09:07:37Z
updated: 2026-09-29T09:07:37Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- cow-wm.codeberg.page via hawski
labels:
- freebsd
- linux
summary: Comments
content: extracted
html: 2026-09-29-cow-a-stacking-window-manager-for-wayland.html
preview:
  file: 2026-09-29-cow-a-stacking-window-manager-for-wayland.preview-22cd696e9e8e.webp
  width: 256
  height: 88
  alt: CoW
  color: '#181717'
images:
- source: https://cow-wm.codeberg.page/cow/assets/CoW-final.svg
  original:
    file: 2026-09-29-cow-a-stacking-window-manager-for-wayland.image-b97663d0bd54.png
    width: 521
    height: 180
  variants:
  - file: 2026-09-29-cow-a-stacking-window-manager-for-wayland.image-b67d3d50c9e0.webp
    width: 320
    height: 111
  - file: 2026-09-29-cow-a-stacking-window-manager-for-wayland.image-413ca57b71fe.webp
    width: 521
    height: 180
  color: '#000000'
- source: https://cow-wm.codeberg.page/cow/assets/cow-ss.png
  original:
    file: 2026-09-29-cow-a-stacking-window-manager-for-wayland.image-cad43e6120ad.png
    width: 1920
    height: 1080
  color: '#282828'
---

CoW — Compositor on Wayland

![CoW](https://cow-wm.codeberg.page/cow/assets/CoW-final.svg)

## A stacking window manager using River as the compositor.

The latest release of CoW is 0.3 (September 2026).

[View the changelog](https://codeberg.org/cow-wm/cow/src/branch/main/CHANGELOG.md)

CoW 0.4 currently includes 23 merged changes since the latest release.

- [PR](https://codeberg.org/cow-wm/cow/pulls/394): cmd: prevent stack
- [PR](https://codeberg.org/cow-wm/cow/pulls/390): module: fix snapshot issues
- [PR](https://codeberg.org/cow-wm/cow/pulls/389): cowpager: inherit font from

[Preview the next release](https://cow-wm.codeberg.page/cow/changelog/development/)

The aims of CoW are to represent the 90s look-and-feel of FVWM and MWM, while also allowing for more modern styles as well. CoW can be configured directly through commands and the same commands can be used in its configuration file, making CoW scriptable through external applications.

High-level features include:

- IPC scripting.
- Server-side decorations can be customisable.
- Placement of windows through different commands.
- Native menu support.
- Rules can control scripting.
- Internal DSL (Domain Specific Language) allows for filtering.

... plus a lot more!

CoW is developed in C, hosted on Codeberg. The community is small but friendly, and we can be found on IRC (irc.libera.chat, in #cow-wayland)

Some useful things to know about the community:

- IRC is the best way of saying hello or asking questions
- Report any issues over IRC or by creating a [Codeberg issue.](https://codeberg.org/cow-wm/cow/issues)
- There's some chatter on Mastodon about CoW and other WMs.
- A community Wiki exists. [Feel free to contribute](https://codeberg.org/cow-wm/cow/wiki)

Development:

- See the [README file](https://codeberg.org/cow-wm/cow)

- For anything else, ask on IRC

[![A CoW desktop showing decorated windows, a container, pager, menus, launcher buttons and iconified windows](https://cow-wm.codeberg.page/cow/assets/cow-ss.png)](https://cow-wm.codeberg.page/cow/assets/cow-ss.png)
