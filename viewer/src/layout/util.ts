/* Layout helpers. This module owns the small number and text helpers that the layout and deck modules share. */

export const clamp = (v: number, a: number, b: number) => Math.max(a, Math.min(b, v));
export const f1 = (v: number) => v.toFixed(1);
/** JetBrains Mono advances 0.6 em per character, so text widths are exact. */
export const mono = (n: number, px: number) => n * 0.6 * px;
/** The map's code text sizes: `--fs-code` and `--fs-code-l` in `deck.css`. */
export const FS_CODE = 11, FS_CODE_L = 11.5;
export const plural = (n: number, one: string) => `${n} ${n === 1 ? one : one + "s"}`;
export const esc = (s: unknown) => String(s).replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!));
