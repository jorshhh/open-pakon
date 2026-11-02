#ifndef PAKON_RENDER_H
#define PAKON_RENDER_H

/* The OEM colour-negative render (tools/pakon_image.py render_oem) in one
 * pass per pixel: density LUT -> NegMatrix -> per-picture adjustments ->
 * rpd.pf 3-D table -> 16-bit sRGB. A shared library loaded by Python
 * (ctypes); no libusb, no protocol. Every step repeats numpy's arithmetic
 * (float32, float64 where numpy promotes, same operation order), so the
 * output is bit-identical to the numpy path. */

#include <stdint.h>

#if defined(_WIN32)
#define PAKON_RENDER_API __declspec(dllexport)
#else
#define PAKON_RENDER_API __attribute__((visibility("default")))
#endif

/* Bumped when the call below changes; Python checks it before using it. */
#define PAKON_RENDER_ABI 1

PAKON_RENDER_API int pakon_render_abi(void);

/* in, out: h x w x 3 uint16, C order. negmat: 3 x 10 float32.
 * lut: 16384 float32 (density LUT). table: n x n x n x 3 float32.
 * contrast: 1000 + contrast / 2 (the slope x 1000), or 0 for none.
 * gain, offset: the balance (3 each), or NULL for none.
 * adjust_off: the red/green/blue + brightness offsets (3), or NULL when the
 * picture has no adjustments. sharp: unsharp amount (sharpness x 0.01), 0
 * for none. grey: average the three RPD channels before the table.
 * Returns 0, or -1 on bad arguments. */
PAKON_RENDER_API int pakon_render_oem(const uint16_t *in, int h, int w,
                                      const float *negmat, const float *lut,
                                      float contrast, const float *gain,
                                      const float *offset, const float *adjust_off,
                                      float sharp, int grey,
                                      const float *table, int n, uint16_t *out);

#endif
