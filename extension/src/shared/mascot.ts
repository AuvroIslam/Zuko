// Zuko's chibi head as an inline SVG, built with createElementNS (no innerHTML, so it works
// under strict page CSPs and Trusted Types). Same palette and shapes as the desktop renderer
// (app/src/character/engine.ts) at a glance size: cream sphere, amber almond eyes, the scar round
// the viewer's right eye, topknot with a red tie and a swooshing ponytail, maroon tunic.

const NS = "http://www.w3.org/2000/svg";
let uid = 0;

type Attrs = Record<string, string | number>;

function el(doc: Document, tag: string, attrs: Attrs = {}, kids: Element[] = []): Element {
  const e = doc.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  for (const c of kids) e.appendChild(c);
  return e;
}

function stop(doc: Document, offset: number, color: string): Element {
  return el(doc, "stop", { offset, "stop-color": color });
}

/** Returns an <svg> (viewBox 0 0 64 64). Give the wrapper CSS the size; the SVG fills it. */
export function mascotSvg(doc: Document): SVGElement {
  const n = ++uid;
  const head = `zk-head-${n}`;
  const eye = `zk-eye-${n}`;
  const defs = el(doc, "defs", {}, [
    el(doc, "radialGradient", { id: head, cx: "36%", cy: "28%", r: "78%" }, [
      stop(doc, 0, "#FFFFFF"),
      stop(doc, 0.3, "#F7F3EE"),
      stop(doc, 0.8, "#E8D5C4"),
      stop(doc, 1, "#CBAE96"),
    ]),
    el(doc, "radialGradient", { id: eye, cx: "50%", cy: "55%", r: "60%" }, [
      stop(doc, 0, "#FFF3CF"),
      stop(doc, 0.45, "#FFB347"),
      stop(doc, 1, "#F28A1E"),
    ]),
  ]);
  const dark = "#2B1D1A";
  const outline = "#1E1310";
  const almond = (cx: number, mirror: number) =>
    el(doc, "path", {
      d: "M-5.2 1.6 C-3 -4.2 2.6 -5.8 5.4 -3 C5.2 2.8 1.2 6 -2 5.6 C-3.8 5.2 -5 3.6 -5.2 1.6Z",
      transform: `translate(${cx} 38) scale(${mirror} 1)`,
      fill: `url(#${eye})`,
      stroke: "#B5400E",
      "stroke-width": 0.8,
    });
  const svg = el(doc, "svg", { viewBox: "0 0 64 64", width: "100%", height: "100%", "aria-hidden": "true", focusable: "false" }, [
    defs,
    // Ponytail swoosh and topknot behind the head.
    el(doc, "path", { d: "M33 13 C41 1 58 5 60 26 C60 33 56 36 55 40 C54 32 52 20 40 17Z", fill: dark, stroke: outline, "stroke-width": 1, "stroke-linejoin": "round" }),
    el(doc, "ellipse", { cx: 32, cy: 10, rx: 6.2, ry: 5, fill: dark, stroke: outline, "stroke-width": 1 }),
    // Maroon tunic with a gold collar.
    el(doc, "path", { d: "M14 64 C15 54 24 52 32 56 C40 52 49 54 50 64Z", fill: "#6E1F18", stroke: outline, "stroke-width": 1, "stroke-linejoin": "round" }),
    el(doc, "path", { d: "M22 54 C27 58 37 58 42 54", fill: "none", stroke: "#E0A030", "stroke-width": 2, "stroke-linecap": "round" }),
    // Head.
    el(doc, "circle", { cx: 32, cy: 35, r: 22, fill: `url(#${head})`, stroke: outline, "stroke-width": 1.6 }),
    // Flame scar round the viewer's right eye.
    el(doc, "path", { d: "M36 40 C33 32 36 26 40 27 C41 25 43 26 43.5 24.5 C46 27 52 29 53 36 C54 44 48 48 42 47 C38 46.5 36.5 43.5 36 40Z", fill: "#B33A2E", opacity: 0.45 }),
    // Eyes.
    almond(23, -1),
    almond(41, 1),
    el(doc, "ellipse", { cx: 22.4, cy: 38, rx: 1.3, ry: 1.7, fill: "#FFFBEA", opacity: 0.85 }),
    el(doc, "ellipse", { cx: 40.4, cy: 38, rx: 1.3, ry: 1.7, fill: "#FFFBEA", opacity: 0.85 }),
    // Highlight.
    el(doc, "ellipse", { cx: 22, cy: 22.5, rx: 6, ry: 3.2, transform: "rotate(-35 22 22.5)", fill: "#FFFFFF", opacity: 0.8 }),
    // Hair-tie.
    el(doc, "rect", { x: 28.4, y: 11.6, width: 7.2, height: 4.6, rx: 1.4, fill: "#B33A2E", stroke: outline, "stroke-width": 0.8 }),
    el(doc, "rect", { x: 28.4, y: 12.8, width: 7.2, height: 0.9, fill: "#E0A030" }),
  ]);
  return svg as SVGElement;
}
