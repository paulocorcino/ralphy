// The odt reader (ADR-0091 D10, amendment 2026-11-23): `content.xml` and
// `styles.xml` of the zip become DOM. Text goes in only as text nodes, an
// image only as a `blob:` address of an image type, and a web link opens in
// a new tab with `noopener noreferrer`.
import { FORMAT_RULES, familyOf, resolveCss, underlineOf } from "./wb-odt-style.ts";
import { zipFile, zipPaths } from "./wb-sheet-read.ts";
import type { Zip } from "./wb-sheet-read.ts";

const NS = {
  office: "urn:oasis:names:tc:opendocument:xmlns:office:1.0",
  text: "urn:oasis:names:tc:opendocument:xmlns:text:1.0",
  table: "urn:oasis:names:tc:opendocument:xmlns:table:1.0",
  draw: "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0",
  style: "urn:oasis:names:tc:opendocument:xmlns:style:1.0",
  svg: "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0",
  xlink: "http://www.w3.org/1999/xlink",
};

const IMAGE_TYPES: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  bmp: "image/bmp",
};

const HEADINGS = ["h1", "h2", "h3", "h4", "h5", "h6"] as const;

type Style = { css: Record<string, string>; parent: string | null };
type Ctx = {
  document: Document;
  styles: Map<string, Style>;
  images: Map<string, string>;
  // The last number of each list style, for a list that continues it.
  counted: Map<string, number>;
};

/** The DOM of an odt file and the CSS of its default paragraph style. */
export type OdtRead = { nodes: Node[]; base: Record<string, string> };

function attr(el: Element, ns: keyof typeof NS, name: string): string | null {
  return el.getAttributeNS(NS[ns], name);
}

// A formatting attribute by its local name. Its namespace is the XSL one of
// the OpenDocument format, its only namespace with ":xsl-" in its URI.
function formatAttr(el: Element, name: string): string | null {
  for (const one of el.attributes) {
    if (one.localName === name && (one.namespaceURI ?? "").includes(":xsl-")) return one.value;
  }
  return null;
}

function fontFamilies(docs: Document[]): Map<string, string> {
  const families = new Map<string, string>();
  for (const doc of docs) {
    for (const face of doc.getElementsByTagNameNS(NS.style, "font-face")) {
      const family = familyOf(attr(face, "svg", "font-family"));
      const name = attr(face, "style", "name");
      if (family !== null && name !== null) families.set(name, family);
    }
  }
  return families;
}

// The CSS of one `style:*-properties` element.
function propsCss(props: Element, families: Map<string, string>): Record<string, string> {
  const css: Record<string, string> = {};
  for (const [name, rule] of Object.entries(FORMAT_RULES)) {
    const raw = formatAttr(props, name);
    const value = raw === null ? null : rule.value(raw);
    if (value !== null) css[rule.css] = value;
  }
  const underline = underlineOf(attr(props, "style", "text-underline-style"));
  if (underline !== null) css["text-decoration"] = underline;
  const family = families.get(attr(props, "style", "font-name") ?? "");
  if (family !== undefined) css["font-family"] = family;
  return css;
}

function styleOf(el: Element, families: Map<string, string>): Style {
  const css: Record<string, string> = {};
  for (const props of el.children) Object.assign(css, propsCss(props, families));
  return { css, parent: attr(el, "style", "parent-style-name") };
}

function readStyles(docs: Document[]) {
  const families = fontFamilies(docs);
  const styles = new Map<string, Style>();
  let base: Record<string, string> = {};
  for (const doc of docs) {
    for (const el of doc.getElementsByTagNameNS(NS.style, "style")) {
      styles.set(attr(el, "style", "name") ?? "", styleOf(el, families));
    }
    for (const el of doc.getElementsByTagNameNS(NS.style, "default-style")) {
      if (attr(el, "style", "family") === "paragraph") base = styleOf(el, families).css;
    }
  }
  return { styles, base };
}

function styled<T extends HTMLElement>(ctx: Ctx, el: T, node: Element): T {
  const name = attr(node, "text", "style-name") ?? attr(node, "table", "style-name");
  for (const [prop, value] of Object.entries(resolveCss(ctx.styles, name))) {
    el.style.setProperty(prop, value);
  }
  return el;
}

function make<K extends keyof HTMLElementTagNameMap>(ctx: Ctx, tag: K, kids: Node[] = []) {
  const el = ctx.document.createElement(tag);
  el.append(...kids);
  return el;
}

function link(ctx: Ctx, node: Element): Node {
  const a = make(ctx, "a", inlines(ctx, node));
  const href = attr(node, "xlink", "href") ?? "";
  if (/^https?:\/\//u.test(href)) {
    a.href = href;
    a.target = "_blank";
    a.rel = "noopener noreferrer";
  }
  return a;
}

function frame(ctx: Ctx, node: Element): Node[] {
  const image = node.getElementsByTagNameNS(NS.draw, "image")[0];
  const url = image ? ctx.images.get(attr(image, "xlink", "href") ?? "") : undefined;
  if (url === undefined) return [];
  const img = make(ctx, "img");
  img.src = url;
  img.style.maxWidth = "100%";
  const width = attr(node, "svg", "width");
  if (width !== null && /^\d+(\.\d+)?(cm|mm|in|pt)$/u.test(width)) img.style.width = width;
  return [img];
}

function spaces(node: Element) {
  return " ".repeat(Math.min(Number(attr(node, "text", "c")) || 1, 200));
}

// One element inside a paragraph.
function inline(ctx: Ctx, node: Element): Node[] {
  switch (node.localName) {
    case "s":
      return [ctx.document.createTextNode(spaces(node))];
    case "tab":
      return [ctx.document.createTextNode("\t")];
    case "line-break":
      return [make(ctx, "br")];
    case "span":
      return [styled(ctx, make(ctx, "span", inlines(ctx, node)), node)];
    case "a":
      return [link(ctx, node)];
    case "frame":
      return frame(ctx, node);
    default:
      return inlines(ctx, node);
  }
}

function inlines(ctx: Ctx, node: Element): Node[] {
  return [...node.childNodes].flatMap((child) => {
    if (child.nodeType === 3) return [ctx.document.createTextNode(child.textContent ?? "")];
    return child.nodeType === 1 && child instanceof Element ? inline(ctx, child) : [];
  });
}

function paragraph(ctx: Ctx, node: Element): Node {
  const level = Number(attr(node, "text", "outline-level")) || 1;
  const tag = node.localName === "h" ? (HEADINGS[level - 1] ?? "h6") : "p";
  const el = styled(ctx, make(ctx, tag, inlines(ctx, node)), node);
  if (el.childNodes.length === 0) el.append(make(ctx, "br"));
  return el;
}

// A list keeps counting from the last list of its style when the file says so.
function list(ctx: Ctx, node: Element): Node {
  const styleName = attr(node, "text", "style-name") ?? "";
  const continues = attr(node, "text", "continue-numbering") === "true";
  const first = continues ? (ctx.counted.get(styleName) ?? 0) + 1 : 1;
  const items = [...node.children].filter((item) => item.localName === "list-item");
  const ol = make(
    ctx,
    "ol",
    items.map((item) => make(ctx, "li", blocks(ctx, item))),
  );
  ol.start = first;
  ctx.counted.set(styleName, first + items.length - 1);
  return ol;
}

function cell(ctx: Ctx, node: Element): Node {
  const td = styled(ctx, make(ctx, "td", blocks(ctx, node)), node);
  const span = Number(attr(node, "table", "number-columns-spanned")) || 1;
  if (span > 1) td.colSpan = Math.min(span, 1000);
  return td;
}

function table(ctx: Ctx, node: Element): Node {
  const rows = [...node.getElementsByTagNameNS(NS.table, "table-row")].map((row) =>
    make(
      ctx,
      "tr",
      [...row.children].filter((c) => c.localName === "table-cell").map((c) => cell(ctx, c)),
    ),
  );
  return make(ctx, "table", [make(ctx, "tbody", rows)]);
}

function block(ctx: Ctx, node: Element): Node[] {
  switch (node.localName) {
    case "p":
    case "h":
      return [paragraph(ctx, node)];
    case "list":
      return [list(ctx, node)];
    case "table":
      return [table(ctx, node)];
    case "section":
      return blocks(ctx, node);
    default:
      return [];
  }
}

function blocks(ctx: Ctx, node: Element): Node[] {
  return [...node.children].flatMap((child) => block(ctx, child));
}

function parseXml(zip: Zip, path: string): Document | null {
  const bytes = zipFile(zip, path);
  if (bytes === null) return null;
  return new DOMParser().parseFromString(new TextDecoder().decode(bytes), "application/xml");
}

function imageUrls(zip: Zip): Map<string, string> {
  const urls = new Map<string, string>();
  for (const path of zipPaths(zip)) {
    const type = IMAGE_TYPES[path.slice(path.lastIndexOf(".") + 1).toLowerCase()];
    if (type === undefined) continue;
    const bytes = zipFile(zip, path);
    if (bytes === null) continue;
    urls.set(path, URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type })));
  }
  return urls;
}

/** The DOM of an odt zip. */
export function readOdt(document: Document, zip: Zip): OdtRead {
  const content = parseXml(zip, "content.xml");
  const body = content?.getElementsByTagNameNS(NS.office, "text")[0];
  if (!content || !body) throw new Error("the odt file has no text body");
  const stylesDoc = parseXml(zip, "styles.xml");
  const { styles, base } = readStyles(stylesDoc ? [stylesDoc, content] : [content]);
  const images = imageUrls(zip);
  const ctx: Ctx = { document, styles, images, counted: new Map() };
  return { nodes: blocks(ctx, body), base };
}
