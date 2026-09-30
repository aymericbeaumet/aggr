---
title: A no-cost audio upgrade for my workstation
link: https://mit.teil.space/rane-sl3.html
source: lobsters
published: 2026-09-30T12:00:03Z
updated: 2026-09-30T12:00:03Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- mit.teil.space via FedericoSchonborn
labels:
- hardware
- nix
summary: Comments
content: extracted
html: 2026-09-30-a-no-cost-audio-upgrade-for-my-workstation.html
---

But now to the real question: how should I test that package? Since the package describes a Linux kernel module, I'd have to add it to my operating system definition, reconfigure my system, reboot it, eliminate bugs and continue this cycle over and over again until it works. But since that machine is operational (and does other tasks in the background) I'd rather not reconfigure my system on top of a source checkout which I would have to roll-back (or pass the \`–allow-downgrades\` flag to a future \`guix pull\` invocation). The good news is: there is no need to go down that route. Instead I do the following:

I adjust the \`bare-bones.tmpl\` minimal operating system example that comes with the Guix source repository to

1. refer to the new package in the \`loadable-kernel-modules\` field and
2. add alsa-utils to the packages field.

```
  modified   gnu/system/examples/bare-bones.tmpl
@@ -4,7 +4,7 @@

 (use-modules (gnu))
 (use-service-modules networking ssh)
-(use-package-modules screen ssh)
+(use-package-modules audio linux screen ssh)

 (operating-system
   (host-name "komputilo")
@@ -20,6 +20,7 @@
   ;; It's fitting to support the equally bare bones ‘-nographic’
   ;; QEMU option, which also nicely sidesteps forcing QWERTY.
   (kernel-arguments (list "console=ttyS0,115200"))
+  (kernel-loadable-modules (list snd-rane-sl3))
   (file-systems (cons (file-system
                         (device (file-system-label "my-root"))
                         (mount-point "/")
@@ -43,7 +44,7 @@
                %base-user-accounts))

   ;; Globally-installed packages.
-  (packages (cons screen %base-packages))
+  (packages (cons* alsa-utils screen %base-packages))

   ;; Add services to the baseline: a DHCP client and an SSH
   ;; server.  You may wish to add an NTP service here.
```

I build an emulation-ready image at the ease of a simple command-line invocation like this:

```
$ ./pre-inst-env guix system vm gnu/system/examples/bare-bones.tmpl
```

from the source checkout where I have the package definition for the kernel module ready. This returns the path to the script that starts that VM on the standard output. This is very convenient, because it allows me to build and start the machine while passing options.

But how can I conduct the test? I have real, physical hardware attached to my workstation and a kernel-module in a virtual machine. Research on the interwebs yields that I need to give myself permission so that \`qemu\` can actually route through USB devices. I do so by changing ownership of the relevant Bus/Device pair (see the output of \`lsusb\` above for the correct numbers) which is in my case:

```
$ sudo chown $(whoami) /dev/bus/usb/003/002
```

After a bunch of trial-and-error iterations I end up with these qemu options:

```
$(./pre-inst-env guix system vm gnu/system/examples/bare-bones.tmpl) \
    -usb \
    -device qemu-xhci \
    -device usb-host,hostbus=003,hostaddr=002 \
    -nographic
```

This launches a non-graphical virtual machine running the GNU system into which I can login simply stating the username \`root'. I check for the USB device, then for the module in the VM.

```
root@komputilo ~# lsusb
Bus 001 Device 001: ID 1d6b:0001 Linux Foundation 1.1 root hub
Bus 002 Device 001: ID 1d6b:0002 Linux Foundation 2.0 root hub
Bus 002 Device 002: ID 1cc5:0001 Rane Corporation SL 3
Bus 003 Device 001: ID 1d6b:0003 Linux Foundation 3.0 root hub
root@komputilo ~# lsmod | grep rane
snd_rane_sl3           40960  0
snd_pcm               188416  1 snd_rane_sl3
snd                   147456  3 snd_timer,snd_rane_sl3,snd_pcm
```

This looks promising! The device was attached, the kernel recognized it and correctly loaded the module to speak to it. Now let's test audio!

```
root@komputilo ~# aplay -l
**** List of PLAYBACK Hardware Devices ****
card 0: RaneSL3 [Rane SL3], device 0: Rane SL3 [Rane SL3]
  Subdevices: 1/1
  Subdevice #0: subdevice #0
```

Awesome: the ALSA utilities recognize the audio device! Now let's hear it. Since I only have two speaker channels set up in my studio (and let's be real: hearing stereo sound to me seems like an adequate test for the kernel module to work) I run \`speaker-test\` with the two channel option after hooking up the first stereo output pair of the sound-card to my sound system:

```
root@komputilo ~# speaker-test -c 2

speaker-test 1.2.11

Playback device is default
Stream parameters are 48000Hz, S16_LE, 2 channels
Using 16 octaves of pink noise
Rate set to 48000Hz (requested 48000Hz)
Buffer size range from 2 to 14563
Period size range from 1 to 7281
Periods = 4
was set period_size = 7281
was set buffer_size = 14563
 0 - Front Left
 1 - Front Right
Time per period = 5.473943
```

What a peaceful, calming sensation that floods my body hearing that pink noise coming out of my speakers. Now all I have to do is to wait for someone (other than myself) in the audio team to approve of my Pull Request, push the change, pull, add the module to my operating system configuration(s), reconfigure, reboot and I will be able to enjoy non-hissy sound!
