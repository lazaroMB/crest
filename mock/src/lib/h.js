// h(tag, props?, ...children): one DOM element.
//
//   h("div", { class: ["row", selected && "row--selected"], onClick: () => … }, "text", child)
//
// `class` takes a string or an array (falsy entries are dropped), `on*` props add
// listeners, `style` takes an object, `true` sets an empty attribute and
// `null`/`false` leave it out. Children may be strings, nodes, arrays or nothing.

const SVG_NS = "http://www.w3.org/2000/svg";

function build(el, props, children) {
  const isProps = props && typeof props === "object" && !(props instanceof Node) && !Array.isArray(props);
  if (!isProps) children = [props, ...children];
  for (const [key, value] of Object.entries(isProps ? props : {})) {
    if (value == null || value === false) continue;
    if (key === "class") {
      el.setAttribute("class", [value].flat().filter(Boolean).join(" "));
    } else if (key.startsWith("on")) {
      el.addEventListener(key.slice(2).toLowerCase(), value);
    } else if (key === "style" && typeof value === "object") {
      Object.assign(el.style, value);
    } else {
      el.setAttribute(key, value === true ? "" : value);
    }
  }
  const nodes = children.flat(Infinity).filter((c) => c != null && c !== false);
  el.append(...nodes.map((c) => (c instanceof Node ? c : String(c))));
  return el;
}

export function h(tag, props, ...children) {
  return build(document.createElement(tag), props, children);
}

export function svg(tag, props, ...children) {
  return build(document.createElementNS(SVG_NS, tag), props, children);
}
