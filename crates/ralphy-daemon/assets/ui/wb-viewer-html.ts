// The body of an HTML file in the rendered pane: a frame with no rights, a
// policy added to the document, and its links rewritten (ADR-0072, amendment
// of 2026-11-15). `wb-viewer-markdown.ts` resolves the images in between.
import { apiFetch } from "./wb-api.ts";
import { markupOf, setMarkup } from "./wb-html.ts";

/**
 * What the document may load: its inline style and `data:` images, and
 * images from the web when the daemon allows them. It sends no other request.
 */
export function framePolicy(remoteImages: boolean): string {
  const images = remoteImages ? "img-src data: https:" : "img-src data:";
  return ["default-src 'none'", "style-src 'unsafe-inline'", images].join("; ");
}

/** Whether the daemon lets a page load images from the web. No answer is a no. */
export async function remoteImagesAllowed(): Promise<boolean> {
  try {
    const reply = await apiFetch("GET /api/security/state");
    return reply.ok ? (await reply.json()).remote_images : false;
  } catch {
    // No answer: the document gets the stricter policy.
    return false;
  }
}

// A bare `#part` is resolved against the URL of the page, and the frame then
// leaves the document (measured: Chromium 148, Firefox 150, WebKit 26.4).
const FRAME_URL = "about:srcdoc";
const XLINK = "http://www.w3.org/1999/xlink";

// A link to a fragment stays, on the frame's own URL. Any other link is a
// GET the frame could send, so it loses its target.
function keepFragmentOnly(link: Element) {
  const href = link.getAttribute("href") ?? "";
  link.removeAttribute("href");
  link.removeAttributeNS(XLINK, "href");
  if (href.startsWith("#")) link.setAttribute("href", FRAME_URL + href);
}

/** Parse the text of an HTML file into the document the frame may show. */
export function frameDocument(
  content: string,
  parser: DOMParser,
  remoteImages: boolean,
): Document {
  const parsed = parser.parseFromString(content, "text/html");
  const policy = parsed.createElement("meta");
  policy.setAttribute("http-equiv", "Content-Security-Policy");
  policy.setAttribute("content", framePolicy(remoteImages));
  // First in the head: a `<meta>` policy covers only what comes after it.
  parsed.head.prepend(policy);
  parsed.querySelectorAll("a, area").forEach((link) => {
    keepFragmentOnly(link);
  });
  return parsed;
}

/** The `srcdoc` text of a document `frameDocument` made. */
export function frameSource(parsed: Document): string {
  const doctype = parsed.doctype ? "<!doctype html>" : "";
  return doctype + markupOf(parsed.documentElement);
}

/** Replace the body of `article` with an empty frame that has no rights. */
export function mountFrame(article: Element, doc: Document): HTMLIFrameElement {
  setMarkup(article, "");
  const frame = doc.createElement("iframe");
  frame.className = "html-frame";
  frame.title = "File preview";
  // Empty on purpose: every right stays off, and none is ever added.
  frame.setAttribute("sandbox", "");
  article.append(frame);
  return frame;
}
