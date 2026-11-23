// Unit tests for assets/ui/wb-viewer-media.ts, the media pane: the element's
// address, the attributes that keep the download and the keys off, and the
// text of an error.
import { test } from "node:test";
import assert from "node:assert/strict";
import { buildMedia, MEDIA_ERROR_TEXT } from "../assets/ui/wb-viewer-media.ts";
import { VIEWERS, viewerKindOf } from "../assets/ui/wb-viewers.ts";
import { VENDOR_FAILURE_TEXT } from "../assets/ui/wb-vendor-load.ts";

// An element that records its attributes, children and listeners, and finds
// the children a selector names by tag, class or data-act.
function fakeEl(tag) {
  const listeners = new Map();
  const el = {
    tag,
    attrs: {},
    style: {},
    dataset: {},
    className: "",
    textContent: "",
    children: [],
    setAttribute(name, value) {
      el.attrs[name] = value;
    },
    append(...kids) {
      el.children.push(...kids);
    },
    addEventListener(name, fn) {
      listeners.set(name, fn);
    },
    fire(name) {
      listeners.get(name)?.({ preventDefault() {} });
    },
    all() {
      return el.children.flatMap((kid) => [kid, ...(kid.all ? kid.all() : [])]);
    },
    querySelector(sel) {
      return stubs.get(sel) ?? null;
    },
  };
  const stubs = new Map();
  el.stub = (sel, child) => {
    stubs.set(sel, child);
    return child;
  };
  return el;
}

function pane(path, load) {
  const mount = fakeEl("viewers");
  const stubs = {};
  const document = {
    createElement: (tag) => {
      const el = fakeEl(tag);
      if (tag === "div") {
        for (const sel of [".media-meta", ".media-body", '[data-act="detach"]', ".viewer-path"]) {
          stubs[sel] = el.stub(sel, fakeEl(sel));
        }
      }
      return el;
    },
  };
  const host = {
    document,
    viewers: mount,
    setPathLabel() {},
    detachBtnHtml: () => "",
    detachClick() {},
  };
  const rec = { id: "t", project: "me/proj", path, checkout: "feat x", content: "", refused: null };
  buildMedia(host, rec, load);
  const media = stubs[".media-body"].all().find((el) => el.tag === "video" || el.tag === "audio");
  return { rec, stubs, media, controller: stubs[".media-body"].children[0] };
}

const loaded = () => Promise.resolve({ ok: true, module: {} });

test("the media element's src is the route address and it has no controls", () => {
  const { media } = pane("clips/a b.mp4", loaded);
  const url = new URL(media.attrs.src, "http://daemon");
  assert.equal(url.pathname, "/api/file/bytes");
  assert.equal(url.searchParams.get("repo"), "me/proj");
  assert.equal(url.searchParams.get("path"), "clips/a b.mp4");
  assert.equal(url.searchParams.get("checkout"), "feat x");
  assert.equal("controls" in media.attrs, false);
  assert.equal("disablepictureinpicture" in media.attrs, true);
  assert.equal(media.tag, "video");
  assert.equal(pane("song.mp3", loaded).media.tag, "audio");
});

test("the controller carries nohotkeys and the bar has no download control", () => {
  const { controller } = pane("a.webm", loaded);
  assert.equal(controller.tag, "media-controller");
  assert.equal("nohotkeys" in controller.attrs, true);
  const tags = controller.all().map((el) => el.tag);
  assert.equal(
    tags.some((tag) => /download/u.test(tag)),
    false,
  );
  assert.equal(tags.includes("media-play-button"), true);
});

test("a media error shows the not-supported message", () => {
  const { media, stubs } = pane("a.wav", loaded);
  media.fire("error");
  assert.equal(stubs[".media-meta"].textContent, MEDIA_ERROR_TEXT);
});

test("a library that fails to load shows the failure text", async () => {
  const { stubs } = pane("a.mp4", () => Promise.resolve({ ok: false, text: VENDOR_FAILURE_TEXT }));
  await new Promise((resolve) => {
    setImmediate(resolve);
  });
  assert.equal(stubs[".media-meta"].textContent, VENDOR_FAILURE_TEXT);
});

test("the table opens the eight media extensions", () => {
  for (const ext of ["mp4", "m4a", "webm", "mp3", "wav", "ogg", "oga", "ogv"]) {
    assert.equal(viewerKindOf(ext), "media", ext);
  }
  assert.equal(VIEWERS.media.build, buildMedia);
});

test("an audio file's controller is in audio mode, a video's is not", () => {
  for (const path of ["voice.ogg", "song.mp3", "a.wav", "a.m4a"]) {
    assert.equal("audio" in pane(path, loaded).controller.attrs, true, path);
  }
  assert.equal("audio" in pane("clip.mp4", loaded).controller.attrs, false);
});
