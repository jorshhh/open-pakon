/*
 * pakon_log.h — tracing/hexdump helpers and the shared result type.
 *
 * This is the lowest utility layer: both the transport (pakon_usb) and the
 * protocol (pakon_proto) layers may include it, but it pulls in neither of
 * them. The shared pakon_result enum lives here so the two layers can speak a
 * common error vocabulary without depending on each other.
 *
 * Logging is gated by the PAKON_DEBUG environment variable, parsed once on
 * first use:
 *
 *   PAKON_DEBUG unset or 0 .. errors only (default)
 *   PAKON_DEBUG=1 .......... + warnings
 *   PAKON_DEBUG=2 .......... + info
 *   PAKON_DEBUG=3 .......... + debug
 *   PAKON_DEBUG=4 .......... + trace (every packet hexdumped)
 */
#ifndef PAKON_LOG_H
#define PAKON_LOG_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Result codes shared across all layers. Negative-free; 0 == success. */
typedef enum {
    PAKON_OK = 0,
    PAKON_ERR_PARAM,          /* bad argument from caller */
    PAKON_ERR_NO_DEVICE,      /* no matching device found */
    PAKON_ERR_USB,            /* libusb-level failure */
    PAKON_ERR_TIMEOUT,        /* transfer timed out */
    PAKON_ERR_PROTO,          /* malformed/unexpected packet */
    PAKON_ERR_STATUS,         /* device returned a non-success status byte */
    PAKON_ERR_FIRMWARE,       /* HEX parse / firmware download failure */
    PAKON_ERR_UNIMPLEMENTED   /* stub not yet implemented (scaffold phase) */
} pakon_result;

/* Human-readable name for a result code (never NULL). */
const char *pakon_result_str(pakon_result r);

typedef enum {
    PAKON_LOG_ERROR = 0,
    PAKON_LOG_WARN  = 1,
    PAKON_LOG_INFO  = 2,
    PAKON_LOG_DEBUG = 3,
    PAKON_LOG_TRACE = 4
} pakon_log_level;

/* Returns nonzero if messages at `level` should be emitted given PAKON_DEBUG. */
int pakon_log_enabled(pakon_log_level level);

/* printf-style log line, emitted to stderr if `level` is enabled. */
void pakon_logf(pakon_log_level level, const char *fmt, ...)
#if defined(__MINGW32__)
    __attribute__((format(gnu_printf, 2, 3)))   /* C99 formats, as stdio */
#elif defined(__GNUC__) || defined(__clang__)
    __attribute__((format(printf, 2, 3)))
#endif
    ;

/*
 * Hexdump `len` bytes of `buf` with a leading `tag`, only if `level` is
 * enabled. Every packet sent to / received from the device routes through
 * this from the very start, so the wire is always inspectable via PAKON_DEBUG.
 */
void pakon_hexdump(pakon_log_level level, const char *tag,
                   const void *buf, size_t len);

#ifdef __cplusplus
}
#endif

#endif /* PAKON_LOG_H */
