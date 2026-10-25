/*
 * pakon_lightcal.c — F-135 line statistics. See pakon_lightcal.h.
 */
#include "pakon_lightcal.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

long pakon_lc_find_origin(const uint16_t *s, size_t nsamples, size_t line_px,
                          size_t black_px, double min_contrast)
{
    size_t L = 4 * line_px, b = black_px;
    size_t lines = L ? nsamples / L : 0;
    if (!s || !b || line_px < 6 * b || lines < 2)
        return -1;
    if (lines > 64)
        lines = 64;
    /* Mean line, then prefix sums over three copies so windows can wrap. */
    double *c = calloc(3 * L + 1, sizeof *c);
    if (!c)
        return -1;
    for (size_t l = 0; l < lines; l++)
        for (size_t i = 0; i < L; i++)
            c[1 + i] += s[l * L + i];
    for (size_t i = 0; i < L; i++)
        c[1 + i] /= (double)lines;
    for (size_t k = 1; k < 3; k++)
        for (size_t i = 0; i < L; i++)
            c[1 + k * L + i] = c[1 + i];
    for (size_t i = 1; i <= 3 * L; i++)
        c[i] += c[i - 1];
#define WIN(o, a, b) ((c[(o) + (b)] - c[(o) + (a)]) / (double)((b) - (a)))
    size_t ir = 3 * line_px;
    double best = -1e18;
    long arg = -1;
    for (size_t o = 0; o < L; o++) {
        /* Whole-block active windows: a lit IR block alone cannot pass for
         * an RGB block, which is three times longer. */
        double sc = WIN(o, 6 * b, ir - 30) - WIN(o, 0, 3 * b) +
                    WIN(o, ir + 3 * b, L - 10) - WIN(o, ir, ir + b);
        if (sc > best) {
            best = sc;
            arg = (long)o;
        }
    }
#undef WIN
    free(c);
    return best >= min_contrast ? arg : -1;
}

long pakon_lc_marker_origin(const uint16_t *s, size_t nsamples, size_t line_len)
{
    size_t lines = line_len ? nsamples / line_len : 0;
    if (!s || lines < 2)
        return -1;
    unsigned *hits = calloc(line_len, sizeof *hits);
    if (!hits)
        return -1;
    for (size_t i = 0; i < lines * line_len; i++)
        if (s[i] & 1u)
            hits[i % line_len]++;
    size_t arg = 0;
    for (size_t i = 1; i < line_len; i++)
        if (hits[i] > hits[arg])
            arg = i;
    unsigned best = hits[arg];
    free(hits);
    return best * 10 >= lines * 6 ? (long)arg : -1;
}

size_t pakon_lc_marker_line_len(const uint16_t *s, size_t nsamples)
{
    size_t prev = 0, best = 0, n = 0, have = 0;
    size_t len[64];
    unsigned cnt[64];
    if (!s)
        return 0;
    for (size_t i = 0; i < nsamples; i++) {
        if (!(s[i] & 1u))
            continue;
        if (have) {
            size_t d = i - prev, k = 0;
            while (k < n && len[k] != d)
                k++;
            if (k == n && n < 64) {
                len[n] = d;
                cnt[n++] = 0;
            }
            if (k < n)
                cnt[k]++;
        }
        prev = i;
        have = 1;
    }
    for (size_t k = 0; k < n; k++)
        if (cnt[k] >= 2 && (!best || cnt[k] > cnt[best - 1]))
            best = k + 1;
    return best ? len[best - 1] : 0;
}

size_t pakon_lc_find_line_px(const uint16_t *s, size_t nsamples, size_t lo,
                             size_t hi, int ir)
{
    size_t nb = PAKON_LC_BLOCKS(ir);
    if (!s || !lo || hi < lo)
        return 0;
    size_t lines = nsamples / (nb * hi);
    if (lines < 4)
        return 0;
    if (lines > 48)
        lines = 48;
    double *m = malloc(nb * hi * sizeof *m);
    if (!m)
        return 0;
    double best = -1;
    size_t arg = 0;
    for (size_t px = lo; px <= hi; px++) {
        size_t L = nb * px;
        memset(m, 0, L * sizeof *m);
        for (size_t l = 0; l < lines; l++)
            for (size_t i = 0; i < L; i++)
                m[i] += s[l * L + i];
        double sum = 0, sq = 0;
        for (size_t i = 0; i < L; i++) {
            sum += m[i];
            sq += m[i] * m[i];
        }
        double var = sq / (double)L - (sum / (double)L) * (sum / (double)L);
        if (var > best) {
            best = var;
            arg = px;
        }
    }
    free(m);
    return arg;
}

int pakon_lc_stats_range(const uint16_t *s, size_t nsamples, size_t line_px,
                         int ir, size_t origin, size_t px0, size_t px1,
                         pakon_lc_stats *out)
{
    if (!s || !out || px1 <= px0 || px1 > line_px)
        return -1;
    memset(out, 0, sizeof *out);
    size_t line = PAKON_LC_BLOCKS(ir) * line_px;
    size_t lines = nsamples > origin ? (nsamples - origin) / line : 0;
    if (!lines)
        return -1;
    size_t npx = px1 - px0;
    double *col = calloc(PAKON_LC_NCH * npx, sizeof *col);
    if (!col)
        return -1;

    for (size_t l = 0; l < lines; l++) {
        const uint16_t *rgb = s + origin + l * line;
        const uint16_t *irb = ir ? rgb + 3 * line_px : NULL;
        for (size_t p = px0; p < px1; p++) {
            col[0 * npx + (p - px0)] += rgb[3 * p];
            col[1 * npx + (p - px0)] += rgb[3 * p + 1];
            col[2 * npx + (p - px0)] += rgb[3 * p + 2];
            if (irb)
                col[3 * npx + (p - px0)] += irb[p];
        }
    }

    out->lines = lines;
    for (int c = 0; c < PAKON_LC_NCH; c++) {
        double sum = 0, best = 0;
        for (size_t i = 0; i < npx; i++) {
            double m = col[c * npx + i] / (double)lines;
            sum += m;
            if (m > best)
                best = m;
        }
        out->mean[c] = sum / (double)npx;
        out->peak[c] = (uint16_t)(best + 0.5);
    }
    free(col);
    return 0;
}

size_t pakon_lc_column_accum(const uint16_t *s, size_t nsamples, size_t line_px,
                             int ir, size_t origin, double *sum, size_t *lines)
{
    if (!s || !sum || !lines || !line_px)
        return 0;
    size_t line = PAKON_LC_BLOCKS(ir) * line_px;
    size_t n = nsamples > origin ? (nsamples - origin) / line : 0;
    for (size_t l = 0; l < n; l++) {
        const uint16_t *rgb = s + origin + l * line;
        const uint16_t *irb = ir ? rgb + 3 * line_px : NULL;
        for (size_t p = 0; p < line_px; p++) {
            sum[0 * line_px + p] += rgb[3 * p];
            sum[1 * line_px + p] += rgb[3 * p + 1];
            sum[2 * line_px + p] += rgb[3 * p + 2];
            if (irb)
                sum[3 * line_px + p] += irb[p];
        }
    }
    *lines += n;
    return n;
}

double pakon_lc_flat_gain(double bright, double dark, double black_bright,
                          double black_dark)
{
    double den = (bright - dark) - (black_bright - black_dark);
    if (den <= 0)
        return 0;
    double g = 64000.0 / den;
    return g > 0x3ffff / 65536.0 ? 0x3ffff / 65536.0 : g;
}

unsigned pakon_lc_smear(double black_bright, double black_dark,
                        double active_bright, double active_dark)
{
    double den = active_bright - active_dark;
    if (den <= 0)
        return 0;
    double v = 65536.0 * (black_bright - black_dark) / den;
    return v >= 1 && v <= 699 ? (unsigned)v : 0;
}

const double pakon_lc_density_c41[PAKON_LC_NCH] = { 0.144, 0.40, 0.715, 0.0 };
const double pakon_lc_density_other[PAKON_LC_NCH] = { 0.0, 0.03, 0.0, 0.08 };
const double pakon_lc_density_slide[PAKON_LC_NCH] = { 0.12, 0.12, 0.12, 0.08 };
const double pakon_lc_density_bwc41[PAKON_LC_NCH] = { 0.10, 0.25, 0.25, 0.08 };

const double *pakon_lc_film_density(pakon_film film)
{
    switch (film) {
    case PAKON_FILM_BW:    return pakon_lc_density_other;
    case PAKON_FILM_SLIDE: return pakon_lc_density_slide;
    case PAKON_FILM_BWC41: return pakon_lc_density_bwc41;
    default:               return pakon_lc_density_c41;
    }
}

const char *pakon_film_name(pakon_film film)
{
    switch (film) {
    case PAKON_FILM_BW:    return "bw";
    case PAKON_FILM_SLIDE: return "slide";
    case PAKON_FILM_BWC41: return "bwc41";
    default:               return "c41";
    }
}

int pakon_film_parse(const char *name, pakon_film *film)
{
    static const pakon_film all[] = { PAKON_FILM_C41, PAKON_FILM_BW, PAKON_FILM_SLIDE,
                                       PAKON_FILM_BWC41 };
    for (size_t i = 0; i < sizeof all / sizeof all[0]; i++)
        if (!strcmp(name, pakon_film_name(all[i]))) {
            *film = all[i];
            return 0;
        }
    return -1;
}

int pakon_lc_offset_next(int offset, double black_mean)
{
    int next = offset + (int)lround((black_mean - PAKON_LC_DARK_TARGET) * -0.0133929);
    /* Outside the window but less than half a code off: the rounded step is
     * 0 and the loop would never move. Step one code toward the target. */
    if (next == offset && !pakon_lc_dark_ok(black_mean))
        next += black_mean > PAKON_LC_DARK_TARGET ? -1 : 1;
    return next < -255 ? -255 : next > 255 ? 255 : next;
}

int pakon_lc_dark_ok(double black_mean)
{
    return fabs(black_mean - PAKON_LC_DARK_TARGET) <= PAKON_LC_DARK_TOL;
}

int pakon_lc_peak_over_cap(int ch, unsigned peak)
{
    static const unsigned cap[PAKON_LC_NCH] = { 64000, 64000, 65500, 40000 };
    return peak > cap[ch];
}

int pakon_lc_peak_ok(int ch, unsigned peak)
{
    unsigned hi = ch == PAKON_LC_IR ? 40000 : 64000;
    return peak >= hi - 64 && peak <= hi;
}

uint16_t pakon_lc_duty_cap(uint16_t period, double density)
{
    return (uint16_t)lround((period - 2) / pow(10.0, density));
}

uint16_t pakon_lc_duty_start(uint16_t cap, unsigned current)
{
    if (current <= 1)
        return (uint16_t)(cap / 2);
    return (uint16_t)lround((double)cap * (current - 1) / current);
}

uint16_t pakon_lc_duty_next(int ch, uint16_t duty, unsigned peak, uint16_t period)
{
    double target = ch == PAKON_LC_IR ? 39968.0 : 63968.0;
    long next = peak ? lround(duty * target / peak) : (long)period - 2;
    if (next < 1)
        next = 1;
    if (next > period - 2)
        next = period - 2;
    return (uint16_t)next;
}

int pakon_lc_duty_settled(int ch, uint16_t duty, unsigned peak, uint16_t period)
{
    if (pakon_lc_peak_ok(ch, peak))
        return 1;
    int next = pakon_lc_duty_next(ch, duty, peak, period);
    return abs(next - (int)duty) <= 1 && next < period - 2;
}

uint16_t pakon_lc_scan_duty(uint16_t open_duty, double density, uint16_t period)
{
    long d = lround(open_duty * pow(10.0, density));
    return (uint16_t)(d > period - 2 ? period - 2 : d);
}
