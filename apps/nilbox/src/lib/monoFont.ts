/**
 * Monospace font handling for the bundled D2Coding webfont (see styles/fonts.css).
 *
 * The CSS variable --font-mono drives every DOM consumer, but xterm.js needs the
 * stack as a plain string (it measures glyph widths itself), so the stacks are
 * mirrored here. Ligatures are opt-in: the preference is stored in localStorage
 * and reflected as data-mono-ligature on <html>, which flips --font-mono.
 *
 * Font size works the same way: stored in localStorage, published as
 * --font-mono-size for the DOM and read directly by xterm.
 */

const LIGATURE_KEY = "nilbox-mono-ligature";
const SIZE_KEY = "nilbox-mono-font-size";
const CHANGE_EVENT = "nilbox-mono-font-change";

export const MONO_FONT_SIZE_DEFAULT = 13;
export const MONO_FONT_SIZE_MIN = 9;
export const MONO_FONT_SIZE_MAX = 24;

export const MONO_FONT_STACK = "'D2Coding', 'Consolas', 'Menlo', 'Monaco', monospace";
export const MONO_FONT_STACK_LIGATURE =
  "'D2Coding Ligature', 'D2Coding', 'Consolas', 'Menlo', 'Monaco', monospace";

export const isMonoLigatureEnabled = (): boolean =>
  localStorage.getItem(LIGATURE_KEY) === "true";

export const monoFontStack = (): string =>
  isMonoLigatureEnabled() ? MONO_FONT_STACK_LIGATURE : MONO_FONT_STACK;

/** Reflects the stored preference onto <html>. Safe to call repeatedly. */
export const applyMonoLigature = (enabled = isMonoLigatureEnabled()): void => {
  const root = document.documentElement;
  if (enabled) root.setAttribute("data-mono-ligature", "on");
  else root.removeAttribute("data-mono-ligature");
};

export const setMonoLigatureEnabled = (enabled: boolean): void => {
  localStorage.setItem(LIGATURE_KEY, String(enabled));
  applyMonoLigature(enabled);
  window.dispatchEvent(new CustomEvent(CHANGE_EVENT));
};

const clampSize = (size: number): number =>
  Math.min(MONO_FONT_SIZE_MAX, Math.max(MONO_FONT_SIZE_MIN, Math.round(size)));

export const monoFontSize = (): number => {
  const stored = parseInt(localStorage.getItem(SIZE_KEY) ?? "", 10);
  return Number.isNaN(stored) ? MONO_FONT_SIZE_DEFAULT : clampSize(stored);
};

/** Publishes the stored size as --font-mono-size. Safe to call repeatedly. */
export const applyMonoFontSize = (size = monoFontSize()): void => {
  document.documentElement.style.setProperty("--font-mono-size", `${clampSize(size)}px`);
};

export const setMonoFontSize = (size: number): void => {
  const next = clampSize(size);
  localStorage.setItem(SIZE_KEY, String(next));
  applyMonoFontSize(next);
  window.dispatchEvent(new CustomEvent(CHANGE_EVENT));
};

/** Notifies non-DOM consumers (xterm) that the mono font or its size changed. */
export const onMonoFontChange = (handler: () => void): (() => void) => {
  window.addEventListener(CHANGE_EVENT, handler);
  return () => window.removeEventListener(CHANGE_EVENT, handler);
};

/**
 * Waits until the bundled faces are actually available. xterm measures glyph
 * width when it opens, so opening before the webfont loads leaves the grid
 * sized for the fallback font until the next resize.
 */
export const ensureMonoFontLoaded = async (): Promise<void> => {
  if (!document.fonts) return;
  const family = isMonoLigatureEnabled() ? "'D2Coding Ligature'" : "'D2Coding'";
  const size = monoFontSize();
  try {
    await Promise.all([
      document.fonts.load(`400 ${size}px ${family}`),
      document.fonts.load(`700 ${size}px ${family}`),
    ]);
  } catch {
    // Font loading failures fall back to the rest of the stack.
  }
};
