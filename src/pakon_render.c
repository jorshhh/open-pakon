#include "pakon_render.h"

#include <stdlib.h>
#include <string.h>

/* numpy rounds every operation on its own; a fused multiply-add would not
 * (CMake also passes -ffp-contract=off, which GCC needs). */
#pragma STDC FP_CONTRACT OFF

#define BAND 64                 /* rows per pass: the band stays in cache */
#define RAW_BLACK 300.0f        /* pakon_image._RAW_BLACK */
#define CONTRAST_PIVOT 1550.0f  /* pakon_image._CONTRAST_PIVOT (0x60E) */

static inline float clipf(float x, float lo, float hi)
{
    /* np.clip: minimum(maximum(x, lo), hi) */
    x = x > lo ? x : lo;
    return x < hi ? x : hi;
}

int pakon_render_abi(void)
{
    return PAKON_RENDER_ABI;
}

/* oem_rpd + _adjust_12bit (all but the unsharp mask) for one pixel. */
static void rpd_pixel(const uint16_t *px, const float *m, const float *lut,
                      float contrast, const float *gain, const float *offset,
                      const float *adjust_off, float *v)
{
    float d[3];
    for (int c = 0; c < 3; c++) {
        float x = clipf(((float)px[c] - RAW_BLACK) / 4.0f, 1.0f, 16383.0f);
        d[c] = lut[(uint16_t)x];
    }
    float r = d[0], g = d[1], b = d[2];
    for (int c = 0; c < 3; c++) {
        const float *k = m + 10 * c;
        float o = k[0] * r + k[1] * g + k[2] * b + k[3] * r * r +
                  k[4] * g * g + k[5] * b * b + k[6] * r * g +
                  k[7] * b * r + k[8] * g * b + k[9];
        o = clipf(o, 0.0f, 4095.0f);
        if (contrast != 0.0f)
            o = clipf((o - CONTRAST_PIVOT) * contrast / 1000.0f + CONTRAST_PIVOT,
                      0.0f, 4095.0f);
        if (gain)
            o = clipf(o * gain[c] + offset[c], 0.0f, 4095.0f);
        if (adjust_off)
            o = clipf(o + adjust_off[c], 0.0f, 4095.0f);
        v[c] = o;
    }
}

/* _interp3 + the 16-bit conversion of render_oem for one pixel. */
static void lookup_pixel(const float *v, const float *table, int n, uint16_t *out)
{
    int i[3];
    double w[2][3];
    for (int c = 0; c < 3; c++) {
        float f = clipf(v[c] / 4095.0f, 0.0f, 1.0f) * (float)(n - 1);
        int k = (int)f;
        i[c] = k < n - 2 ? k : n - 2;
        w[1][c] = (double)f - (double)i[c];   /* float32 - int32: numpy's float64 */
        w[0][c] = 1.0 - w[1][c];
    }
    const float *t = table + 3 * (((size_t)i[0] * n + i[1]) * n + i[2]);
    double acc[3] = {0.0, 0.0, 0.0};
    for (int dr = 0; dr < 2; dr++)
        for (int dg = 0; dg < 2; dg++) {
            double wrg = w[dr][0] * w[dg][1];
            for (int db = 0; db < 2; db++) {
                double ww = wrg * w[db][2];
                const float *e = t + 3 * (((size_t)dr * n + dg) * n + db);
                for (int c = 0; c < 3; c++)
                    acc[c] = acc[c] + ww * (double)e[c];
            }
        }
    for (int c = 0; c < 3; c++) {
        float y = clipf((float)acc[c] * 65535.0f + 0.5f, 0.0f, 65535.0f);
        out[c] = (uint16_t)y;
    }
}

int pakon_render_oem(const uint16_t *in, int h, int w,
                     const float *negmat, const float *lut,
                     float contrast, const float *gain,
                     const float *offset, const float *adjust_off,
                     float sharp, int grey,
                     const float *table, int n, uint16_t *out)
{
    if (!in || !out || !negmat || !lut || !table || h <= 0 || w <= 0 || n < 2 ||
        (gain && !offset))
        return -1;
    size_t row = (size_t)w * 3;
    int halo = sharp != 0.0f ? 1 : 0;
    float *band = malloc(sizeof(float) * row * (BAND + 2));   /* RPD, halo rows too */
    float *sh = malloc(sizeof(float) * row * 2);              /* sharpened row, column blur */
    if (!band || !sh) {
        free(band);
        free(sh);
        return -1;
    }
    for (int r0 = 0; r0 < h; r0 += BAND) {
        int r1 = r0 + BAND < h ? r0 + BAND : h;
        int a = r0 - halo > 0 ? r0 - halo : 0;
        int b = r1 + halo < h ? r1 + halo : h;
        for (int y = a; y < b; y++)
            for (int x = 0; x < w; x++)
                rpd_pixel(in + (size_t)y * row + 3 * x, negmat, lut, contrast,
                          gain, offset, adjust_off, band + (size_t)(y - a) * row + 3 * x);
        for (int y = r0; y < r1; y++) {
            float *v = band + (size_t)(y - a) * row;
            if (halo) {
                /* _unsharp: x + amount * (x - blur), blur the separable
                 * [1/4, 1/2, 1/4] kernel (rows first), edges repeated. */
                const float *up = band + (size_t)((y > 0 ? y - 1 : 0) - a) * row;
                const float *dn = band + (size_t)((y < h - 1 ? y + 1 : h - 1) - a) * row;
                float *rows = sh + row;
                for (size_t k = 0; k < row; k++)
                    rows[k] = 0.25f * up[k] + 0.5f * v[k] + 0.25f * dn[k];
                for (int x = 0; x < w; x++) {
                    int xl = x > 0 ? x - 1 : 0, xr = x < w - 1 ? x + 1 : w - 1;
                    for (int c = 0; c < 3; c++) {
                        float blur = 0.25f * rows[3 * xl + c] + 0.5f * rows[3 * x + c] +
                                     0.25f * rows[3 * xr + c];
                        float s = v[3 * x + c];
                        sh[3 * x + c] = clipf(s + sharp * (s - blur), 0.0f, 4095.0f);
                    }
                }
                v = sh;
            }
            for (int x = 0; x < w; x++) {
                float p[3] = {v[3 * x], v[3 * x + 1], v[3 * x + 2]};
                if (grey) {
                    float mean = (p[0] + p[1] + p[2]) / 3.0f;
                    p[0] = p[1] = p[2] = mean;
                }
                lookup_pixel(p, table, n, out + (size_t)y * row + 3 * x);
            }
        }
    }
    free(band);
    free(sh);
    return 0;
}
