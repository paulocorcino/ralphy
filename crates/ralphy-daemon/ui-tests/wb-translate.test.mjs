// Unit tests for assets/ui/wb-translate.js — runs the real source with no DOM
// and no real Chrome. The file is a classic <script> that assigns
// window.WBTranslate; we execute it through `new Function(...)` with injected
// fakes for window/self/navigator/Translator/LanguageDetector.
//
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of assets/ui
// into the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-translate.js"),
  "utf8",
);

// Load a fresh WBTranslate with the given fake globals. Any global left
// undefined is simply absent from `self` (mirrors a browser lacking the API).
function load({ self: selfObj, navigator, Translator, LanguageDetector } = {}) {
  const window = {};
  const selfArg = selfObj === undefined ? {} : selfObj;
  const navArg = navigator === undefined ? { language: "en" } : navigator;
  new Function(
    "window",
    "self",
    "navigator",
    "Translator",
    "LanguageDetector",
    SRC,
  )(window, selfArg, navArg, Translator, LanguageDetector);
  return window.WBTranslate;
}

test("supported() false when self has no Translator", () => {
  assert.equal(load({ self: {} }).supported(), false);
});

test("supported() true when self has Translator", () => {
  assert.equal(load({ self: { Translator: {} } }).supported(), true);
});

test("progressText renders a rounded percent label", () => {
  const wb = load();
  assert.equal(wb.progressText(0.42), "downloading model — 42%");
  assert.equal(wb.progressText(1), "downloading model — 100%");
});

test("explainError maps the generic Chromium failure to an actionable string", () => {
  const wb = load();
  assert.equal(
    wb.explainError("Other generic failures occurred.", "pt", "en"),
    "couldn't download the pt→en model — check your connection and free disk space",
  );
});

test("explainError passes an already-actionable message through unchanged", () => {
  const wb = load();
  assert.equal(
    wb.explainError("no pt→en model available on this device", "pt", "en"),
    "no pt→en model available on this device",
  );
});

test("decide: null source fails with the no-source message", () => {
  const wb = load();
  assert.deepEqual(wb.decide(null, "en", "available"), {
    action: "fail",
    message: "couldn't detect the source language — translation skipped",
  });
});

test("decide: unavailable model fails with the no-model message", () => {
  const wb = load();
  assert.deepEqual(wb.decide("pt", "en", "unavailable"), {
    action: "fail",
    message: "no pt→en model available on this device",
  });
});

test("decide: downloadable model routes to download", () => {
  const wb = load();
  assert.equal(wb.decide("pt", "en", "downloadable").action, "download");
});

test("decide: same source and target is a no-op", () => {
  const wb = load();
  assert.equal(wb.decide("en", "en", "available").action, "same");
});

test("decide: available model routes to translate", () => {
  const wb = load();
  assert.equal(wb.decide("pt", "en", "available").action, "translate");
});

test("translate: fires onProgress and resolves during a model download", async () => {
  const Translator = {
    availability: async () => "downloadable",
    create: async (opts) => {
      const listeners = [];
      opts.monitor({
        addEventListener: (ev, cb) => listeners.push([ev, cb]),
      });
      listeners.forEach(
        ([ev, cb]) => ev === "downloadprogress" && cb({ loaded: 0.42 }),
      );
      return { translate: async () => "TRANSLATED", destroy() {} };
    },
  };
  const LanguageDetector = {
    create: async () => ({
      detect: async () => [{ detectedLanguage: "pt" }],
      destroy() {},
    }),
  };
  const wb = load({ self: { Translator, LanguageDetector }, Translator, LanguageDetector });
  const seen = [];
  const res = await wb.translate("olá", "en", (msg) => seen.push(msg));
  assert.deepEqual(seen, ["downloading model — 42%"]);
  assert.deepEqual(res, { text: "TRANSLATED", source: "pt", target: "en", same: false });
});

test("translate: a generic create failure rejects with the mapped actionable string", async () => {
  const Translator = {
    availability: async () => "downloadable",
    create: async () => {
      throw new Error("Other generic failures occurred.");
    },
  };
  const LanguageDetector = {
    create: async () => ({
      detect: async () => [{ detectedLanguage: "pt" }],
      destroy() {},
    }),
  };
  const wb = load({ self: { Translator, LanguageDetector }, Translator, LanguageDetector });
  await assert.rejects(wb.translate("olá", "en", () => {}), {
    message: "couldn't download the pt→en model — check your connection and free disk space",
  });
});

test("translate: a failed source detection rejects and never requests a model pair", async () => {
  let availabilityCalls = 0;
  let createCalls = 0;
  const Translator = {
    availability: async () => {
      availabilityCalls++;
      return "available";
    },
    create: async () => {
      createCalls++;
      return { translate: async () => "x", destroy() {} };
    },
  };
  // no LanguageDetector in self → detect() yields null
  const wb = load({ self: { Translator }, Translator });
  await assert.rejects(wb.translate("x", "en"), {
    message: "couldn't detect the source language — translation skipped",
  });
  assert.equal(availabilityCalls, 0);
  assert.equal(createCalls, 0);
});
