---
title: 'Nura (postmarketOS): The road to daily-drivable mainline phones'
link: https://postmarketos.org/blog/2026/09/29/road-to-main-category/
source: lobsters
published: 2026-09-29T19:20:05Z
updated: 2026-09-29T19:20:05Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- postmarketos.org via achill
labels:
- linux
- mobile
summary: Comments
content: extracted
html: 2026-09-29-nura-postmarketos-the-road-to-daily-drivable-mainline-phones.html
preview:
  file: 2026-09-29-nura-postmarketos-the-road-to-daily-drivable-mainline-phones.preview-d0ed28137e96.webp
  width: 256
  height: 120
  color: '#656f5b'
images:
- source: https://postmarketos.org/static/img/2026-09/main-device-roadmap.jpg
  original:
    file: 2026-09-29-nura-postmarketos-the-road-to-daily-drivable-mainline-phones.image-b3e22f8efadb.jpg
    width: 1500
    height: 704
  color: '#293734'
- source: https://nura.eco/static/img/2026-09/main-device-roadmap.jpg
  original:
    file: 2026-09-29-nura-postmarketos-the-road-to-daily-drivable-mainline-phones.image-b3e22f8efadb.jpg
    width: 1500
    height: 704
  color: '#293734'
---

![Radxa Dragon Q6A, Fairphone 5 and Motorola Edge 30 on top of a Nura-stickered laptop](https://nura.eco/static/img/2026-09/main-device-roadmap.jpg)

We have shown that running mainline Linux on your phone is a real possibility for highly invested Linux enthusiasts. Now how do we get from there to making it usable for everybody else who just wants a working phone?

Two important segments of the road towards this destination are [Duranium](https://nura.eco/blog/2026/03/17/introducing-duranium/) and [Hardware CI](https://nura.eco/blog/2026/01/21/hw-ci-mvp/). This blog post is about the third one: **reference devices!**

Members of the Nura team have joined forces to build maintainer teams for three of the many devices Nura runs on to push them across the finishing line and make them suitable for everyday use with Nura. More on the actual workflow comes further below, let's start with defining the goal in detail.

## [New "main" category](https://postmarketos.org/blog/2026/09/29/road-to-main-category/#new-main-category)

We [categorize devices](https://docs.nura.eco/pmaports/main/packaging/device-categorization.html) into "main", "community", "testing", "downstream" and "archived". The "main" category was [emptied](https://nura.eco/blog/2024/12/23/v24.12-release/#pinephone-and-librem-5) with the v24.12 release. With [PMCR-0009](https://docs.nura.eco/pmcr/main/0009-new-main-device-category.html) we have re-evaluated what we want to have in the "main" device category. Here is the summary:

> Set new requirements for the “main” device category to highlight selected device ports which are well-tested in hardware CI and set up to stay in “main” for a long time through strong maintainership.
>
> Change the meaning of the “main” category to not only indicate that more features are working than in the “community” category, but also that the Nura team is highly invested in keeping the device in the “main” category and takes on responsibilities to make this likely.
>
> Maintainers of devices in other categories are welcome to use some of these new requirements for “main” as blueprint for their devices as well, in order to get similar reliability and maintainership improvements for their devices.

### [Fully mainline](https://postmarketos.org/blog/2026/09/29/road-to-main-category/#fully-mainline)

After many discussions (the PMCR merge request had 151 comments), we have arrived at [high quality requirements](https://docs.nura.eco/pmaports/main/packaging/device-categorization.html#main) for ports in this category. Among others:

- Boot via UEFI (e.g. through a second-stage bootloader on phones).
- Must use [upstream kernels](https://docs.nura.eco/pmaports/main/packaging/kernel-packages/generic-kernel-packages.html) with a strict and minimal [policy for patches](https://docs.nura.eco/pmaports/main/packaging/kernel-packages/generic-kernel-packages.html#policy-for-patches).
- Must not depend on forked device-specific packages, such as `alsa-ucm-conf`.
- Must use a generic device package for the target architecture.

This means that the resulting ports are essentially fully mainlined and can not only be used with Nura, but also relatively easily with any other Linux distribution. There will be one UI-specific aarch64 image that can be flashed on all "main" aarch64 devices. Getting Linux kernel security patches will be trivial, as we only need to update our generic kernel packages and then get them for all devices in the "main" category at once.

### [Device features](https://postmarketos.org/blog/2026/09/29/road-to-main-category/#device-features)

Regarding device features, "main" category requirements now have:

> **The working features should allow to use the device in most common use cases.** A phone for example would typically have calls, SMS, mobile data, Wi-Fi, audio, battery charging, Bluetooth and camera. Exceptions can be made by the device maintainer team, together with reasoning why they are necessary (e.g. fingerprint reader is not working because the driver is missing). The Nura team decides if the port is complete enough for the main category based on that list.

### [Device maintainer team](https://postmarketos.org/blog/2026/09/29/road-to-main-category/#device-maintainer-team)

In order to pull this off, each device must have a team of maintainers that consists of at least 5 people, of which the majority are part of the [Nura team](https://nura.eco/team/). Between these people, a list of responsibilities must be covered. As with the other requirements listed above, this is an ideal the team would be working towards for eventually getting the device into *main*. The team can consist of fewer people and have a smaller scope initially.

From the [list of responsibilities](https://docs.nura.eco/pmaports/main/packaging/device-categorization.html#main), most importantly:

- Organize regular meetings.
- Long-term commitment for the device.
- Kernel maintenance (fixing regressions on the kernel side, new kernel developments).
- Triage issues found by the community and HW CI regressions.
- Documentation for this device.
- Making sure Hardware CI works (wires are connected, preparing CI).
- Manual testing where necessary.

## [Workflow](https://postmarketos.org/blog/2026/09/29/road-to-main-category/#workflow)

So how can your favorite device get into the main category? We have thought hard about this and came up with the following workflow:

- Become part of a team of device maintainers through issues in the [new-device-teams](https://gitlab.postmarketos.org/postmarketOS/new-device-teams/-/work_items) project. You can either apply to join an existing team by commenting in an existing issue or create a new one.

- When creating a new issue, the [Nura infrastructure team](https://docs.nura.eco/policies-and-processes/governance/groups-and-teams.html#infrastructure-team) will create bridged Matrix and IRC channels for you, and a pmaports label for this new device will be created. (This is a manual process, if we don't do this within a week then please kindly ask in the devel chat.)

- Wait until you have at least two people in the potential new team, then find a meeting time that works for everyone and start doing regular meetings. Use the meetings to figure out how to implement the requirements for the main category.

- Once all requirements for *main* are fulfilled (this will take quite some time, but the device port will already improve significantly in this process!), make a merge request to move the device to the "main" category.

## [Financing](https://postmarketos.org/blog/2026/09/29/road-to-main-category/#financing)

Most of the work done in Nura is volunteer-based. Therefore, we cannot really promise ETAs for this project. Still, donations make it possible to finance development and HW-CI hardware. In some specific cases we might even be able to directly fund development work (e.g. [q6voice(d)](https://nura.eco/blog/2026/05/08/q6voice-project/)) too. We are also working on applying for grants to potentially support part of this project.

If you are interested in supporting this project, you can make sure that some of your [donations](https://nura.eco/donate) will go specifically to this project! If you want to get in touch for some bigger-targeted donations to directly support development, we would also be happy to hear from you at `board at postmarketos dot org` (emails are not migrated to nura.eco yet).

## [Initial candidates](https://postmarketos.org/blog/2026/09/29/road-to-main-category/#initial-candidates)

Together with this blog post, we have created three initial issues in the new-device-teams project:

- [Fairphone 5](https://gitlab.postmarketos.org/postmarketOS/new-device-teams/-/work_items/1)
- [Motorola Edge 30](https://gitlab.postmarketos.org/postmarketOS/new-device-teams/-/work_items/3)
- [Radxa Dragon Q6A](https://gitlab.postmarketos.org/postmarketOS/new-device-teams/-/work_items/2)

All of these are based on the [SM7325](https://wiki.nura.eco/wiki/Qualcomm_Snapdragon_778G/778G%2B/782G_\(SM7325\)) SoC for which significant mainline support exists already, to the point that we believe there is a good chance to eventually fulfill all requirements needed for the new main category. For all of these we are already able to use UART.

The Radxa Dragon Q6A is a single-board computer, which means it will be much easier to get this moved to main first compared to actual phones. Fairphone as OEM is ideologically very aligned with our project, while the Edge 30 is a cheaper phone that is easier to obtain in some regions.

## [Get involved](https://postmarketos.org/blog/2026/09/29/road-to-main-category/#get-involved)

Now it's your turn. If you would like to see one of these devices become well maintained in Nura to the point that you can daily drive them without making compromises, consider joining their device maintainer teams. You don't even need to be a programmer to help out, there are many non-coding tasks such as testing, organization, triaging issues etc. that are super important as well and ensure that the programmers don't burn out.

If you are significantly interested in improving another device port (even if the end-goal is not main), look through the [existing issues](https://gitlab.postmarketos.org/postmarketOS/new-device-teams/-/work_items). If it is not there, consider creating a [new issue](https://gitlab.postmarketos.org/postmarketOS/new-device-teams/-/work_items/new) and get the ball rolling.

This blog post was written by [Pablo](https://nura.eco/team/#pablo-correa-gomez-pabloyoyoista) and [Oliver](https://nura.eco/team/#oliver-smith-ollieparanoid).
