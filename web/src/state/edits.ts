// Adjust's edits on a frame, kept on its box (Crop.e) and sent with the
// export; the service saves them beside the raw with the boxes.

// In slider clicks (0 = none): colour pairs (+ = red / green / blue),
// density (+ = darker, a minilab's D), contrast, saturation; rot = degrees
// clockwise, then the flips. Sharpening is the roll's (Adjust's menu).
export interface Edit { cr: number; mg: number; yb: number; dn: number; ct: number; sat: number; rot: number; fh: boolean; fv: boolean }
export const noEdit = (): Edit => ({ cr: 0, mg: 0, yb: 0, dn: 0, ct: 0, sat: 0, rot: 0, fh: false, fv: false });

// TLX units per click (Config/ColorCorrection/defaults.ini): 25 for the
// colour offsets, brightness and contrast, saturation in its own −5..5
// steps. Density is TLX brightness the other way round.
const UNIT = { cr: 25, mg: 25, yb: 25, dn: -25, ct: 25, sat: 1 } as const;
const KEY = { cr: 'red', mg: 'green', yb: 'blue', dn: 'brightness', ct: 'contrast', sat: 'saturation' } as const;
export type Slider = keyof typeof UNIT;
const SLIDERS = Object.keys(UNIT) as Slider[];

// The edit as the service takes it (only what is set); `colour` false
// leaves out everything but the turn and flips.
export function editJSON(e?: Edit, colour = true): Record<string, number | boolean> {
  const out: Record<string, number | boolean> = {};
  if (!e) return out;
  if (colour) for (const k of SLIDERS) if (e[k]) out[KEY[k]] = Math.round(e[k] * UNIT[k]);
  if (e.rot) out.rotate = e.rot;
  if (e.fh) out.flip_h = true;
  if (e.fv) out.flip_v = true;
  return out;
}

// An edit saved by the service, back in clicks.
export function editFrom(j?: Record<string, number | boolean>): Edit | undefined {
  if (!j || !Object.keys(j).length) return undefined;
  const e = noEdit();
  for (const k of SLIDERS) e[k] = Math.round(Number(j[KEY[k]] || 0) / UNIT[k]) || 0;
  e.rot = Number(j.rotate || 0);
  e.fh = !!j.flip_h;
  e.fv = !!j.flip_v;
  return e;
}

export const edited = (e?: Edit) => !!e && Object.keys(editJSON(e)).length > 0;
