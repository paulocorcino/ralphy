// The media pane (ADR-0091 D12): a native audio or video element on the file
// bytes route, inside media-chrome controls. The element has no `controls`
// and the controller has `nohotkeys`, so the browser's own bar (which offers
// a download in Firefox) and the library's keys stay off.
import { setMarkup } from "./wb-html.ts";
import { need, required } from "./wb-dom-need.ts";
import { fileBytesUrl } from "./wb-file-bytes.ts";
import { loadVendor } from "./wb-vendor-load.ts";
import type { VendorLoad } from "./wb-vendor-load.ts";
import type { ViewerReadOnlyDeps } from "./wb-file-viewer-read-only.ts";
import type { BytesRecord } from "./wb-file-viewer-records.ts";

export const MEDIA_CHROME_PATH = "./vendor/media-chrome/media-chrome.mjs";
export const MEDIA_ERROR_TEXT = "This browser cannot play this file.";

const VIDEO_EXTENSIONS = ["mp4", "webm", "ogv"];

function isVideo(path: string) {
  const ext = path.slice(path.lastIndexOf(".") + 1).toLowerCase();
  return VIDEO_EXTENSIONS.includes(ext);
}

function element(document: Document, tag: string, attrs: Record<string, string> = {}) {
  const el = document.createElement(tag);
  for (const [name, value] of Object.entries(attrs)) el.setAttribute(name, value);
  return el;
}

// The controls of one controller: play, seek, time, mute and volume, and
// full screen for a video. None of them saves the file.
function controlBar(document: Document, video: boolean) {
  const bar = element(document, "media-control-bar");
  bar.append(
    element(document, "media-play-button"),
    element(document, "media-time-range"),
    element(document, "media-time-display", { showduration: "" }),
    element(document, "media-mute-button"),
    element(document, "media-volume-range"),
  );
  if (video) bar.append(element(document, "media-fullscreen-button"));
  return bar;
}

// Without `audio`, media-chrome sizes an audio element as a video with no
// picture: the pane is empty and the bar hides itself.
function mediaController(document: Document, media: HTMLElement, video: boolean) {
  const attrs: Record<string, string> = { nohotkeys: "", class: "media-controller" };
  if (!video) attrs["audio"] = "";
  const controller = element(document, "media-controller", attrs);
  controller.style.width = "100%";
  controller.style.maxHeight = "100%";
  controller.append(media, controlBar(document, video));
  return controller;
}

function mediaElement(document: Document, url: string, video: boolean) {
  const media = element(document, video ? "video" : "audio", {
    slot: "media",
    src: url,
    preload: "metadata",
    controlslist: "nodownload noremoteplayback",
    disablepictureinpicture: "",
  });
  media.addEventListener("contextmenu", (ev: Event) => {
    ev.preventDefault();
  });
  return media;
}

export function buildMedia(
  host: ViewerReadOnlyDeps,
  rec: BytesRecord,
  load: (path: string) => Promise<VendorLoad> = loadVendor,
) {
  const { document, setPathLabel, detachBtnHtml } = host;
  const el = document.createElement("div");
  el.className = "viewer media-viewer";
  el.dataset["tabId"] = rec.id;
  el.style.display = "none";
  setMarkup(
    el,
    `
    <div class="viewer-toolbar">
      <span class="viewer-path"></span>
      <span class="media-meta"></span>
      <span class="spacer"></span>
      ${detachBtnHtml(rec)}
    </div>
    <div class="viewer-body media-body"></div>`,
  );
  setPathLabel(el, rec);
  required(host.viewers, "the element that holds the panes").append(el);

  const video = isVideo(rec.path);
  const media = mediaElement(document, fileBytesUrl(rec.project, rec.path, rec.checkout), video);
  const meta = need(el, ".media-meta");
  media.addEventListener("error", () => {
    meta.textContent = MEDIA_ERROR_TEXT;
  });
  need(el, ".media-body").append(mediaController(document, media, video));
  need(el, '[data-act="detach"]').addEventListener("click", () => {
    host.detachClick(rec);
  });
  void load(MEDIA_CHROME_PATH).then((outcome) => {
    if (!outcome.ok) meta.textContent = outcome.text;
    return outcome;
  });
  rec.el = el;
}
