import { createAudio } from "./audio";
import { enhanceInteractive } from "./interactive";
import { createNativeTiming, createProviderTiming, finiteDuration, formatPlaybackEnd, playbackTiming, updateConsumption, type PlaybackTimingState } from "./media-timing";

export interface MediaOptions {
  base(): string;
  kind(): string;
}

export function createMedia(options: MediaOptions) {
  const audio = createAudio();
  let videoObserver: ResizeObserver | undefined;
  let listeners = new AbortController();
  const timedMedia = new Map<Element, () => void>();

  function enhancePreview(root: ParentNode = document) {
    for (const media of root.querySelectorAll<HTMLElement>(".preview-media")) {
      if (media.dataset.previewBound === "true") continue;
      const image = media.querySelector<HTMLImageElement>(".preview-image");
      if (!image) continue;
      media.dataset.previewBound = "true";
      if (image.complete) {
        media.classList.add("is-loaded");
        if (!image.naturalWidth) media.classList.add("is-error");
        continue;
      }
      media.classList.add("is-loading");
      image.addEventListener("load", () => media.classList.add("is-loaded"), { once: true, signal: listeners.signal });
      image.addEventListener("error", () => media.classList.add("is-loaded", "is-error"), { once: true, signal: listeners.signal });
    }
  }

  function timingDisplay(player: Element) {
    const sibling = player.nextElementSibling;
    const host = sibling?.matches("[data-media-timing]") ? sibling : null;
    const root = player.closest("article.item") || document;
    return {
      update(state: PlaybackTimingState) {
        if (state.duration) updateConsumption(root, "watch", state.duration);
        const { endsAt } = playbackTiming(state);
        const text = endsAt === undefined ? "" : `Ends at ${formatPlaybackEnd(endsAt)}`;
        if (host && host.textContent !== text) host.textContent = text;
      },
      clear() { if (host) host.textContent = ""; }
    };
  }

  function enhanceNativeVideo(root: ParentNode) {
    for (const video of root.querySelectorAll<HTMLVideoElement>(".native-video video")) {
      if (timedMedia.has(video)) continue;
      const display = timingDisplay(video.closest(".native-video")!);
      const timing = createNativeTiming(video, display.update, finiteDuration(Number(video.dataset.durationSeconds)));
      timedMedia.set(video, () => { timing.dispose(); display.clear(); });
    }
  }

  function enhanceArticle(root: ParentNode = document) {
    audio.enhance(root);
    enhanceNativeVideo(root);
    enhanceInteractive(root, listeners.signal);
    for (const media of root.querySelectorAll<HTMLElement>(".article-picture, .article-lead")) {
      if (media.dataset.articleMediaBound === "true") continue;
      const image = media.querySelector<HTMLImageElement>(".progressive-image, img");
      if (!image) continue;
      media.dataset.articleMediaBound = "true";
      const signal = listeners.signal;

      function loaded() {
        if (signal.aborted) return;
        media.classList.add("is-loaded");
        media.removeAttribute("aria-busy");
      }
      function failed() {
        if (signal.aborted) return;
        media.classList.remove("is-loading");
        media.classList.add("is-error");
        media.removeAttribute("aria-busy");
      }

      if (image.complete) {
        if (image.naturalWidth) loaded();
        else failed();
        continue;
      }
      media.classList.add("is-loading");
      media.setAttribute("aria-busy", "true");
      image.addEventListener("load", () => {
        if (typeof image.decode === "function") void image.decode().catch(() => {}).then(loaded);
        else loaded();
      }, { once: true, signal });
      image.addEventListener("error", failed, { once: true, signal });
    }
  }

  // Every provider player, including videos linked inside article bodies, is a facade that loads
  // the provider only when the reader activates it, and then starts playback at once.
  function enhanceVideo(root: ParentNode = document) {
    for (const preview of root.querySelectorAll<HTMLAnchorElement>("[data-video-embed]")) enhanceVideoFacade(preview);
  }

  function enhanceVideoFacade(preview: HTMLAnchorElement) {
    const player = preview.closest<HTMLElement>(".video-player");
    if (!player || player.dataset.videoMounted === "true" || player.dataset.videoBound === "true") return;
    const provider = player.dataset.videoProvider;
    if (provider === "twitch" && location.protocol !== "https:"
      && !["localhost", "127.0.0.1", "[::1]"].includes(location.hostname)) return;
    player.dataset.videoBound = "true";
    const inline = player.classList.contains("video-player-inline");

    function mountPlayer(autoplay: boolean) {
      if (!player || player.dataset.videoMounted === "true") return;
      player.dataset.videoMounted = "true";
      const url = new URL(preview.dataset.videoEmbed || "");
      if (preview.hasAttribute("data-video-parent")) url.searchParams.set("parent", location.hostname);
      url.searchParams.set("autoplay", provider === "twitch" ? String(autoplay) : autoplay ? "1" : "0");
      if (provider === "youtube") {
        url.searchParams.set("enablejsapi", "1");
        url.searchParams.set("origin", location.origin);
      }
      const frame = document.createElement("iframe");
      frame.src = url.href;
      frame.title = preview.getAttribute("aria-label") || "";
      frame.loading = autoplay ? "eager" : "lazy";
      frame.allow = "autoplay; encrypted-media; fullscreen; picture-in-picture";
      frame.allowFullscreen = true;
      frame.referrerPolicy = "strict-origin-when-cross-origin";
      frame.setAttribute("sandbox", "allow-scripts allow-same-origin allow-presentation");
      player.classList.add("is-loading");
      player.setAttribute("aria-busy", "true");
      frame.addEventListener("load", () => {
        player.classList.add("is-loaded");
        player.removeAttribute("aria-busy");
        preview.hidden = true;
      }, { once: true, signal: listeners.signal });
      if (!inline && (provider === "youtube" || provider === "vimeo")) {
        const display = timingDisplay(player);
        const timing = createProviderTiming({
          provider, origin: url.origin,
          duration: finiteDuration(Number(player.dataset.durationSeconds)),
          source: () => frame.contentWindow,
          post: message => frame.contentWindow?.postMessage(provider === "youtube" ? JSON.stringify(message) : message, url.origin),
          update: display.update
        });
        window.addEventListener("message", timing.message, { signal: listeners.signal });
        frame.addEventListener("load", timing.loaded, { signal: listeners.signal });
        timedMedia.set(frame, () => { timing.dispose(); display.clear(); });
      }
      player.appendChild(frame);
      if (provider === "twitch") {
        let measuredWidth = 0;
        function resize() {
          if (!player) return;
          const available = player.clientWidth;
          if (!available || available === measuredWidth) return;
          measuredWidth = available;
          // Twitch needs a 400×300 player viewport even on narrower phones.
          const width = Math.max(400, available);
          frame.style.width = `${width}px`;
          frame.style.height = `${width * 3 / 4}px`;
          frame.style.transform = `scale(${available / width})`;
        }
        resize();
        if (window.ResizeObserver) {
          videoObserver = new ResizeObserver(resize);
          videoObserver.observe(player);
        }
      }
      if (autoplay) frame.focus({ preventScroll: true });
    }
    preview.setAttribute("role", "button");
    preview.addEventListener("keydown", (event) => {
      if (event.key === " ") { event.preventDefault(); preview.click(); }
    }, { signal: listeners.signal });
    preview.addEventListener("click", (event) => {
      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      event.preventDefault();
      mountPlayer(true);
    }, { signal: listeners.signal });
  }

  // A page lifecycle can reuse this service after releasing the replaced page's resources.
  async function dispose() {
    const audioDisposal = audio.dispose();
    for (const release of timedMedia.values()) release();
    timedMedia.clear();
    listeners.abort();
    listeners = new AbortController();
    videoObserver?.disconnect();
    videoObserver = undefined;
    for (const media of document.querySelectorAll<HTMLElement>("[data-preview-bound]")) delete media.dataset.previewBound;
    for (const media of document.querySelectorAll<HTMLElement>("[data-article-media-bound]")) delete media.dataset.articleMediaBound;
    for (const player of document.querySelectorAll<HTMLElement>("[data-video-bound]")) delete player.dataset.videoBound;
    for (const link of document.querySelectorAll<HTMLElement>("[data-interactive-bound]")) delete link.dataset.interactiveBound;
    await audioDisposal;
  }

  return { enhancePreview, enhanceArticle, enhanceVideo, dispose };
}
