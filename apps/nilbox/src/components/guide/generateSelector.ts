export function generateSelector(el: Element): { selector: string; stable: boolean } {
  // Priority 1: data-guide-id on element itself
  const guideId = el.getAttribute("data-guide-id");
  if (guideId) return { selector: `[data-guide-id='${guideId}']`, stable: true };

  // Priority 2: unique id
  if (el.id && document.querySelectorAll(`#${CSS.escape(el.id)}`).length === 1) {
    return { selector: `#${CSS.escape(el.id)}`, stable: true };
  }

  // Priority 3: nearest ancestor with data-guide-id + nth-child path
  let ancestor = el.parentElement;
  while (ancestor && ancestor !== document.body) {
    const ancestorGuideId = ancestor.getAttribute("data-guide-id");
    if (ancestorGuideId) {
      const path = buildPathTo(el, ancestor);
      if (path) return { selector: `[data-guide-id='${ancestorGuideId}'] ${path}`, stable: true };
    }
    ancestor = ancestor.parentElement;
  }

  // Priority 4: structural path (unstable fallback)
  const path = buildStructuralPath(el, 4);
  return { selector: path, stable: false };
}

function buildPathTo(target: Element, ancestor: Element): string | null {
  const parts: string[] = [];
  let current: Element | null = target;
  while (current && current !== ancestor) {
    const p: Element | null = current.parentElement;
    if (!p) return null;
    const index = Array.from(p.children).indexOf(current) + 1;
    parts.unshift(`${current.tagName.toLowerCase()}:nth-child(${index})`);
    current = p;
  }
  return parts.join(" > ");
}

function buildStructuralPath(el: Element, maxDepth: number): string {
  const parts: string[] = [];
  let current: Element | null = el;
  let depth = 0;
  while (current && current !== document.body && depth < maxDepth) {
    const p: Element | null = current.parentElement;
    if (!p) break;
    const index = Array.from(p.children).indexOf(current) + 1;
    const className = current.className && typeof current.className === "string"
      ? `.${current.className.trim().split(/\s+/)[0]}`
      : "";
    parts.unshift(`${current.tagName.toLowerCase()}${className}:nth-child(${index})`);
    current = p;
    depth++;
  }
  return parts.join(" > ");
}
