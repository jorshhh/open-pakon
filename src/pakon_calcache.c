/*
 * pakon_calcache.c — recalibration policy and the saved calibration
 * (see pakon_calcache.h).
 *
 * File: magic, a byte-order word, then the fields as fixed-width host-order
 * values and the two tables as doubles. It is a local cache, never shared,
 * so host order is enough; another order or version is just a miss.
 */
#include "pakon_calcache.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const char CAL_MAGIC[8] = { 'P', 'K', 'C', 'A', 'L', '0', '0', '1' };
#define CAL_BOM 0x01020304u
#define CAL_MAX_LINE_PX 16384u

const char *pakon_cal_action_name(pakon_cal_action a)
{
    switch (a) {
    case PAKON_CAL_REUSE:   return "reuse";
    case PAKON_CAL_REFRESH: return "refresh";
    default:                return "full";
    }
}

pakon_cal_action pakon_cal_needed(int64_t now, int64_t when, int64_t when_full)
{
    if (when <= 0 || when_full <= 0 || when > now || when_full > now ||
        now - when_full > PAKON_CAL_FULL_S)
        return PAKON_CAL_FULL;
    if (now - when > PAKON_CAL_INTERVAL_S)
        return PAKON_CAL_REFRESH;
    return PAKON_CAL_REUSE;
}

int pakon_cal_path(char *buf, size_t size, const char *dir, pakon_film film,
                   int base, int ir)
{
    char base_part[16] = "";
    if (base == 8 || base == 4)
        snprintf(base_part, sizeof base_part, "-base%d", base);
    else if (base != 16)
        return -1;
    int n = snprintf(buf, size, "%s/calibration-%s%s%s.bin", dir,
                     pakon_film_name(film), base_part, ir ? "" : "-noir");
    return n < 0 || (size_t)n >= size ? -1 : 0;
}

static int put32(FILE *f, int32_t v) { return fwrite(&v, sizeof v, 1, f) == 1 ? 0 : -1; }
static int put64(FILE *f, int64_t v) { return fwrite(&v, sizeof v, 1, f) == 1 ? 0 : -1; }
static int get32(FILE *f, int32_t *v) { return fread(v, sizeof *v, 1, f) == 1 ? 0 : -1; }
static int get64(FILE *f, int64_t *v) { return fread(v, sizeof *v, 1, f) == 1 ? 0 : -1; }

int pakon_cal_save(const char *path, const pakon_cal_record *rec)
{
    if (!path || !rec || !rec->dark || !rec->bright || !rec->line_px)
        return -1;
    /* Write a temporary file and rename it, so a crash never leaves half a
     * record behind under the real name. */
    char tmp[1100];
    int n = snprintf(tmp, sizeof tmp, "%s.tmp", path);
    if (n < 0 || (size_t)n >= sizeof tmp)
        return -1;
    FILE *f = fopen(tmp, "wb");
    if (!f)
        return -1;
    int bad = fwrite(CAL_MAGIC, sizeof CAL_MAGIC, 1, f) != 1;
    bad |= put32(f, (int32_t)CAL_BOM);
    bad |= put32(f, (int32_t)rec->serial);
    bad |= put32(f, (int32_t)rec->offset);
    bad |= put32(f, (int32_t)rec->film);
    bad |= put64(f, rec->when);
    bad |= put64(f, rec->when_full);
    for (int c = 0; c < 3; c++)
        bad |= put32(f, rec->gain[c]) | put32(f, rec->off[c]);
    for (int c = 0; c < 4; c++)
        bad |= put32(f, (int32_t)rec->cur[c]) | put32(f, rec->duty[c]);
    bad |= put64(f, (int64_t)rec->line_px);
    bad |= put64(f, (int64_t)rec->lines_dark);
    bad |= put64(f, (int64_t)rec->lines_bright);
    size_t nt = PAKON_LC_NCH * rec->line_px;
    bad |= fwrite(rec->dark, sizeof *rec->dark, nt, f) != nt;
    bad |= fwrite(rec->bright, sizeof *rec->bright, nt, f) != nt;
    bad |= ferror(f);
#ifdef _WIN32
    /* Windows' rename() does not replace an existing file */
    if (!bad)
        remove(path);
#endif
    if (fclose(f) || bad || rename(tmp, path)) {
        remove(tmp);
        return -1;
    }
    return 0;
}

int pakon_cal_load(const char *path, unsigned serial, unsigned offset,
                   pakon_film film, size_t line_px, pakon_cal_record *rec)
{
    if (!path || !rec)
        return -1;
    memset(rec, 0, sizeof *rec);
    FILE *f = fopen(path, "rb");
    if (!f)
        return -1;
    char magic[sizeof CAL_MAGIC];
    int32_t bom, v32[4];
    int64_t v64[3];
    int bad = fread(magic, sizeof magic, 1, f) != 1 ||
              memcmp(magic, CAL_MAGIC, sizeof magic) ||
              get32(f, &bom) || (uint32_t)bom != CAL_BOM;
    for (int i = 0; !bad && i < 3; i++)
        bad = get32(f, &v32[i]);
    if (!bad) {
        rec->serial = (unsigned)v32[0];
        rec->offset = (unsigned)v32[1];
        rec->film = (pakon_film)v32[2];
        bad = get64(f, &rec->when) || get64(f, &rec->when_full);
    }
    for (int c = 0; !bad && c < 3; c++) {
        bad = get32(f, &v32[0]) || get32(f, &v32[1]);
        rec->gain[c] = v32[0];
        rec->off[c] = v32[1];
    }
    for (int c = 0; !bad && c < 4; c++) {
        bad = get32(f, &v32[0]) || get32(f, &v32[1]);
        rec->cur[c] = (unsigned)v32[0];
        rec->duty[c] = (uint16_t)v32[1];
    }
    for (int i = 0; !bad && i < 3; i++)
        bad = get64(f, &v64[i]);
    if (!bad) {
        rec->line_px = (size_t)v64[0];
        rec->lines_dark = (size_t)v64[1];
        rec->lines_bright = (size_t)v64[2];
        bad = rec->serial != serial || rec->offset != offset || rec->film != film ||
              (line_px && rec->line_px != line_px) || rec->line_px == 0 ||
              rec->line_px > CAL_MAX_LINE_PX;
    }
    if (!bad) {
        size_t nt = PAKON_LC_NCH * rec->line_px;
        rec->dark = malloc(nt * sizeof *rec->dark);
        rec->bright = malloc(nt * sizeof *rec->bright);
        bad = !rec->dark || !rec->bright ||
              fread(rec->dark, sizeof *rec->dark, nt, f) != nt ||
              fread(rec->bright, sizeof *rec->bright, nt, f) != nt;
    }
    fclose(f);
    if (bad) {
        pakon_cal_free(rec);
        return -1;
    }
    return 0;
}

void pakon_cal_free(pakon_cal_record *rec)
{
    if (!rec)
        return;
    free(rec->dark);
    free(rec->bright);
    rec->dark = rec->bright = NULL;
}
