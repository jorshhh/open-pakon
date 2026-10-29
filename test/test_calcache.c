/*
 * test_calcache — recalibration policy (TLB.dll FN_bBeforeScan) and the
 * saved calibration record (no hardware).
 */
#include "pakon_calcache.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#ifdef _WIN32
#include <direct.h>
#include <io.h>         /* _mktemp */
static char *mkdtemp(char *t) { return _mktemp(t) && _mkdir(t) == 0 ? t : NULL; }
#define TMP_DIR "test_calcacheXXXXXX"      /* in the current folder */
#else
#define TMP_DIR "/tmp/test_calcacheXXXXXX"
#endif

static int failures = 0;

#define CHECK(cond, msg) do {                                   \
        if (cond) { printf("ok   - %s\n", (msg)); }             \
        else { printf("FAIL - %s\n", (msg)); failures++; }      \
    } while (0)

int main(void)
{
    /* ---- policy */
    const int64_t now = 1800000000;
    CHECK(pakon_cal_needed(now, 0, 0) == PAKON_CAL_FULL, "no record: full");
    CHECK(pakon_cal_needed(now, now, now) == PAKON_CAL_REUSE, "just calibrated: reuse");
    CHECK(pakon_cal_needed(now, now - 3600, now - 3600) == PAKON_CAL_REUSE,
          "60 min exactly: reuse (OEM recalibrates only past the interval)");
    CHECK(pakon_cal_needed(now, now - 3601, now - 3601) == PAKON_CAL_REFRESH,
          "past 60 min: refresh");
    CHECK(pakon_cal_needed(now, now - 600, now - 72000) == PAKON_CAL_REUSE,
          "20 h since full, 10 min since last: reuse");
    CHECK(pakon_cal_needed(now, now - 600, now - 72001) == PAKON_CAL_FULL,
          "past 20 h since full: full even if recent");
    CHECK(pakon_cal_needed(now, now - 7200, now - 50000) == PAKON_CAL_REFRESH,
          "2 h since last, 14 h since full: refresh");
    CHECK(pakon_cal_needed(now, now + 10, now - 100) == PAKON_CAL_FULL,
          "record from the future: full");
    CHECK(!strcmp(pakon_cal_action_name(PAKON_CAL_REFRESH), "refresh"), "action name");

    /* ---- file round trip */
    char dir[] = TMP_DIR;
    if (!mkdtemp(dir)) {
        printf("FAIL - mkdtemp\n");
        return 1;
    }
    char path[1024];
    CHECK(pakon_cal_path(path, sizeof path, dir, PAKON_FILM_BW, 16, 0) == 0 &&
          strstr(path, "/calibration-bw-noir.bin"), "path: Base 16, IR off");
    CHECK(pakon_cal_path(path, sizeof path, dir, PAKON_FILM_BW, 4, 0) == 0 &&
          strstr(path, "/calibration-bw-base4-noir.bin"), "path: Base 4, IR off");
    CHECK(pakon_cal_path(path, sizeof path, dir, PAKON_FILM_BW, 12, 1) == -1, "bad base");
    CHECK(pakon_cal_path(path, sizeof path, dir, PAKON_FILM_BW, 16, 1) == 0 &&
          strstr(path, "/calibration-bw.bin"), "Base 16 + IR keeps the old name");
    char tiny[8];
    CHECK(pakon_cal_path(tiny, sizeof tiny, dir, PAKON_FILM_BW, 16, 1) == -1,
          "path too long");
    CHECK(pakon_cal_path(path, sizeof path, dir, PAKON_FILM_BW, 8, 1) == 0 &&
          strstr(path, "/calibration-bw-base8.bin"), "path per film and base");

    enum { LP = 37 };
    pakon_cal_record rec = {
        .serial = 3054, .offset = 43, .film = PAKON_FILM_BW,
        .when = now - 5, .when_full = now - 500,
        .gain = { 13, 14, 15 }, .off = { -38, -31, -30 },
        .cur = { 2, 3, 3, 2 }, .duty = { 896, 708, 278, 1353 },
        .line_px = LP, .lines_dark = 128, .lines_bright = 130,
    };
    rec.dark = malloc(PAKON_LC_NCH * LP * sizeof *rec.dark);
    rec.bright = malloc(PAKON_LC_NCH * LP * sizeof *rec.bright);
    for (int i = 0; i < PAKON_LC_NCH * LP; i++) {
        rec.dark[i] = 300.5 + i;
        rec.bright[i] = 64000.25 - i;
    }
    rec.when = now - 50;
    CHECK(pakon_cal_save(path, &rec) == 0, "save");
    rec.when = now - 5;
    CHECK(pakon_cal_save(path, &rec) == 0, "save again (replaces the file)");

    pakon_cal_record got;
    int r = pakon_cal_load(path, 3054, 43, PAKON_FILM_BW, LP, &got);
    CHECK(r == 0, "load");
    if (r == 0) {
        CHECK(got.when == rec.when && got.when_full == rec.when_full, "timestamps");
        CHECK(!memcmp(got.gain, rec.gain, sizeof rec.gain) &&
              !memcmp(got.off, rec.off, sizeof rec.off), "A/D gains and offsets");
        CHECK(!memcmp(got.cur, rec.cur, sizeof rec.cur) &&
              !memcmp(got.duty, rec.duty, sizeof rec.duty), "currents and duties");
        CHECK(got.lines_dark == 128 && got.lines_bright == 130, "table line counts");
        CHECK(!memcmp(got.dark, rec.dark, PAKON_LC_NCH * LP * sizeof *rec.dark) &&
              !memcmp(got.bright, rec.bright, PAKON_LC_NCH * LP * sizeof *rec.bright),
              "tables");
        pakon_cal_free(&got);
    }
    CHECK(pakon_cal_load(path, 3055, 43, PAKON_FILM_BW, LP, &got) == -1 && !got.dark,
          "other serial: miss");
    CHECK(pakon_cal_load(path, 3054, 44, PAKON_FILM_BW, LP, &got) == -1, "other offset: miss");
    CHECK(pakon_cal_load(path, 3054, 43, PAKON_FILM_C41, LP, &got) == -1, "other film: miss");
    CHECK(pakon_cal_load(path, 3054, 43, PAKON_FILM_BW, LP + 1, &got) == -1,
          "other line length: miss");
    CHECK(pakon_cal_load(path, 3054, 43, PAKON_FILM_BW, 0, &got) == 0 &&
          got.line_px == LP && got.dark && got.dark[LP] == rec.dark[LP],
          "line length 0: the saved one");
    pakon_cal_free(&got);

    /* Truncated file. */
    FILE *f = fopen(path, "r+b");
    if (f) {
        fseek(f, 0, SEEK_END);
        long size = ftell(f);
        fclose(f);
        CHECK(truncate(path, size - 8) == 0 &&
              pakon_cal_load(path, 3054, 43, PAKON_FILM_BW, LP, &got) == -1,
              "truncated: miss");
    }
    remove(path);
    CHECK(pakon_cal_load(path, 3054, 43, PAKON_FILM_BW, LP, &got) == -1, "missing: miss");

    pakon_cal_free(&rec);
    CHECK(pakon_cal_save(path, &rec) == -1, "save without tables refused");
    rmdir(dir);

    printf("%s\n", failures ? "FAILED" : "all passed");
    return failures ? 1 : 0;
}
