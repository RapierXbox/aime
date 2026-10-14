// mail is dark-safe if it opts in (color-scheme meta/property, prefers-color-scheme
// query) or sets no colors at all, so system text color can't clash with it.
// ponytail: regex heuristic on raw HTML; misses colors set via class-only external CSS
export const supportsDarkMode = (html: string) =>
  /color-scheme[^;>{}]*dark/i.test(html) ||
  !/(?:^|[\s;"'{])(?:color|background(?:-color)?)\s*:|bgcolor\s*=|<font[^>]*\bcolor\s*=/i.test(html);
