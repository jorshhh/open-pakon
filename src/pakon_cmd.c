#define _POSIX_C_SOURCE 200809L   /* nanosleep */
#include "pakon_cmd.h"

#include <string.h>
#include <time.h>

pakon_result pakon_cmd(pakon_dev *dev, const pakon_packet *cmd,
                       pakon_packet *reply, unsigned timeout_ms)
{
    if (!dev || !cmd)
        return PAKON_ERR_PARAM;

    uint8_t wire[PAKON_PACKET_SIZE];
    size_t wlen = 0;
    pakon_result r = pakon_packet_serialize(cmd, wire, sizeof(wire), &wlen);
    if (r != PAKON_OK)
        return r;

    size_t sent = 0;
    r = pakon_usb_send(dev, PAKON_EP_CMD_OUT, wire, wlen, &sent, timeout_ms);
    if (r != PAKON_OK)
        return r;
    if (sent != wlen)
        return PAKON_ERR_USB;

    if (!reply)
        return PAKON_OK;

    /* Replies are short frames; a max-struct-sized buffer is plenty. */
    uint8_t buf[PAKON_PACKET_SIZE];
    size_t got = 0;
    r = pakon_usb_recv(dev, PAKON_EP_CMD_IN, buf, sizeof(buf), &got, timeout_ms);
    if (r != PAKON_OK)
        return r;

    return pakon_packet_parse(reply, buf, got);
}

pakon_result pakon_cmd_raw(pakon_dev *dev, uint8_t type,
                           const uint8_t *data, size_t dlen,
                           pakon_packet *reply, unsigned timeout_ms)
{
    pakon_packet cmd;
    pakon_result r = pakon_packet_build(&cmd, type, data, dlen);
    if (r != PAKON_OK)
        return r;
    return pakon_cmd(dev, &cmd, reply, timeout_ms);
}

void pakon_bridge_open(pakon_dev *dev, unsigned timeout_ms)
{
    /* HostReset: 04 03 10 00 85 */
    const uint8_t host_reset[] = {AD_HOST, 0x00, 0x85};
    /* HostSetMode: 02 04 10 01 8f 00 */
    const uint8_t host_set_mode[] = {AD_HOST, 0x01, 0x8f, 0x00};
    pakon_packet reply;

    /* Reply timeouts are expected on every open after the first in a power
     * session (see header); results are deliberately ignored. */
    (void)pakon_cmd_raw(dev, PH_CMD, host_reset, sizeof(host_reset),
                        &reply, timeout_ms);
    (void)pakon_cmd_raw(dev, PH_WRITE, host_set_mode, sizeof(host_set_mode),
                        &reply, timeout_ms);
}

pakon_pic_state pakon_probe_pic(pakon_dev *dev, uint8_t pic_address,
                                uint8_t *status_out, unsigned timeout_ms)
{
    const uint8_t probe[] = {pic_address, 0x00, 0x00};
    pakon_packet reply;

    if (status_out)
        *status_out = PS_NONE;
    if (pakon_cmd_raw(dev, PH_CMD, probe, sizeof(probe), &reply,
                      timeout_ms) != PAKON_OK)
        return PAKON_PIC_ERROR;
    if (reply.type != PH_RESPONSE || reply.count < 2 ||
        pakon_packet_addr(&reply) != pic_address)
        return PAKON_PIC_ERROR;

    uint8_t status = pakon_packet_status(&reply);
    if (status_out)
        *status_out = status;
    if (status == PS_SUCCESS)
        return PAKON_PIC_PRESENT;
    if (status == PS_NOT_ACKED)
        return PAKON_PIC_ABSENT;
    return PAKON_PIC_ERROR;
}

#define BUSY_POLL_TRIES  44
#define REPLY_TRIES      3

static void sleep_ms(unsigned ms)
{
    struct timespec ts = { ms / 1000, (long)(ms % 1000) * 1000000L };
    nanosleep(&ts, NULL);
}

static int is_bootloader(uint8_t a)
{
    return a == AD_BOOT_PICL || a == AD_BOOT_PICM ||
           a == AD_BOOT_PICL_PLUS || a == AD_BOOT_PICM_PLUS;
}

/* Flags byte of a READ/STATUS reply: error bits (0x04 does not count from
 * the host). */
static int flags_error(uint8_t addr, uint8_t flags)
{
    return (flags & 0x20) || (addr != AD_HOST && (flags & 0x04));
}

/* Send one frame with the reply rules; `*reply_out` (may be NULL) gets the
 * accepted reply. */
static pakon_result run_one(pakon_dev *dev, const pakon_packet *pkt,
                            unsigned timeout_ms, pakon_packet *reply_out)
{
    uint8_t addr = pkt->data[0];
    pakon_packet buf;
    pakon_packet *reply = reply_out ? reply_out : &buf;
    pakon_result r;

    if (pkt->type == PH_READ_STATUS) {
        for (unsigned t = 1; t <= BUSY_POLL_TRIES; t++) {
            r = pakon_cmd(dev, pkt, reply, timeout_ms);
            if (r != PAKON_OK)
                return r;
            if (reply->type != PH_READ_STATUS || reply->count < 2 ||
                pakon_packet_addr(reply) != addr)
                return PAKON_ERR_PROTO;
            uint8_t flags = reply->data[1];
            if (flags_error(addr, flags))
                return PAKON_ERR_STATUS;
            if (!(flags & 0x01))
                return PAKON_OK;
            sleep_ms(t);   /* growing delay */
        }
        pakon_logf(PAKON_LOG_ERROR, "busy poll 0x%02x: still busy after %d tries",
                   addr, BUSY_POLL_TRIES);
        return PAKON_ERR_TIMEOUT;
    }

    for (unsigned t = 1; t <= REPLY_TRIES; t++) {
        r = pakon_cmd(dev, pkt, reply, timeout_ms);
        if (r != PAKON_OK)
            return r;
        if (reply->count < 2 || pakon_packet_addr(reply) != addr)
            return PAKON_ERR_PROTO;
        uint8_t st = reply->data[1];
        if (pkt->type == PH_READ) {
            if (reply->type != PH_READ)
                return PAKON_ERR_PROTO;
            if (flags_error(addr, st))
                return PAKON_ERR_STATUS;
            if (!(st & 0x11))
                return PAKON_OK;
        } else {
            if (reply->type != PH_RESPONSE)
                return PAKON_ERR_PROTO;
            if (st == PS_SUCCESS || st == PS_SUCCESS_8)
                return PAKON_OK;
            if (st != PS_BAD_CHECKSUM && st != PS_USB_6 && st != PS_BUS_ERROR)
                return PAKON_ERR_STATUS;
        }
        pakon_logf(PAKON_LOG_WARN, "frame to 0x%02x: status 0x%02x, retry %u",
                   addr, st, t);
    }
    return PAKON_ERR_STATUS;
}

/* The reply to `01 03 <low> 1e 90`: log the DX sensor block and add it to
 * `roll` (may be NULL). */
static void take_dx_block(const pakon_packet *reply, pakon_dx_roll *roll)
{
    pakon_dx_block b;
    char line[384];

    if (reply->count < 2 + PAKON_DX_REPLY_LEN ||
        pakon_dx_parse(reply->data + 2, reply->count - 2, &b) != 0) {
        pakon_hexdump(PAKON_LOG_WARN, "dx: unparsed sensor block",
                      reply->data, reply->count);
        return;
    }
    pakon_logf(PAKON_LOG_INFO, "dx: %s", pakon_dx_format(&b, line, sizeof line));
    if (roll)
        pakon_dx_roll_add(roll, &b);
}

static int is_dx_read(const pakon_packet *p, uint8_t low)
{
    return p->type == PH_READ && p->count >= 3 && p->data[0] == low &&
           p->data[1] == PAKON_DX_REPLY_LEN && p->data[2] == 0x90;
}

/* pakon_cmd_run_seq; with `low` nonzero, DX sensor block reads from it go to
 * take_dx_block. */
static pakon_result run_seq(pakon_dev *dev, const pakon_seq *seq,
                            unsigned timeout_ms, uint8_t low,
                            pakon_dx_roll *roll)
{
    if (!dev || !seq || seq->overflow)
        return PAKON_ERR_PARAM;
    /* Safety rules: check the whole sequence before sending any of it. */
    for (size_t i = 0; i < seq->n; i++) {
        const pakon_packet *p = &seq->pkt[i];
        if (p->type == PH_INVALID || p->count == 0 || is_bootloader(p->data[0]))
            return PAKON_ERR_PARAM;
    }
    for (size_t i = 0; i < seq->n; i++) {
        pakon_packet reply;
        if (seq->delay_ms[i])
            sleep_ms(seq->delay_ms[i]);
        pakon_result r = run_one(dev, &seq->pkt[i], timeout_ms, &reply);
        if (r != PAKON_OK) {
            pakon_logf(PAKON_LOG_ERROR, "sequence stopped at frame %zu: %s",
                       i, pakon_result_str(r));
            return r;
        }
        if (low && is_dx_read(&seq->pkt[i], low))
            take_dx_block(&reply, roll);
    }
    return PAKON_OK;
}

pakon_result pakon_cmd_run_seq(pakon_dev *dev, const pakon_seq *seq,
                               unsigned timeout_ms)
{
    return run_seq(dev, seq, timeout_ms, 0, NULL);
}

pakon_result pakon_cmd_service_events(pakon_dev *dev, uint8_t low, uint8_t scn,
                                      unsigned timeout_ms, uint8_t *host_flags,
                                      unsigned *serviced)
{
    return pakon_cmd_service_events_dx(dev, low, scn, timeout_ms, host_flags,
                                       serviced, NULL);
}

/* The event service; `low_status` (may be NULL) collects the bits of every
 * LOW event status acknowledged. */
static pakon_result service_events(pakon_dev *dev, uint8_t low, uint8_t scn,
                                   unsigned timeout_ms, uint8_t *host_flags,
                                   unsigned *serviced, pakon_dx_roll *dx,
                                   uint8_t *low_status)
{
    const uint8_t poll[1] = { AD_HOST };
    pakon_packet pkt, reply;
    pakon_result r;

    if (!dev)
        return PAKON_ERR_PARAM;
    if ((r = pakon_packet_build(&pkt, PH_READ_STATUS, poll, 1)) != PAKON_OK ||
        (r = pakon_cmd(dev, &pkt, &reply, timeout_ms)) != PAKON_OK)
        return r;
    if (reply.type != PH_READ_STATUS || reply.count < 2 ||
        pakon_packet_addr(&reply) != AD_HOST)
        return PAKON_ERR_PROTO;
    uint8_t flags = reply.data[1];
    if (host_flags)
        *host_flags = flags;
    if (flags & PAKON_HOST_FLAG_OVERFLOW)
        pakon_logf(PAKON_LOG_WARN, "event service: host reports FIFO overflow");
    if (!(flags & PAKON_HOST_FLAG_EVENT))
        return PAKON_OK;

    const uint8_t addrs[2] = { low, scn };   /* LOW first, as the OEM does */
    for (int i = 0; i < 2; i++) {
        uint8_t status = 0;
        pakon_event_read_frame(&pkt, addrs[i]);
        if ((r = pakon_cmd(dev, &pkt, &reply, timeout_ms)) != PAKON_OK)
            return r;
        if (!pakon_event_parse(&reply, addrs[i], &status))
            continue;
        pakon_seq seq;
        pakon_seq_init(&seq);
        pakon_event_followup(&seq, addrs[i], status, addrs[i] == low);
        if ((r = run_seq(dev, &seq, timeout_ms, low, dx)) != PAKON_OK)
            return r;
        pakon_logf(PAKON_LOG_INFO, "event service: 0x%02x status 0x%02x acknowledged",
                   addrs[i], status);
        if (serviced)
            (*serviced)++;
        if (low_status && addrs[i] == low)
            *low_status |= status;
    }
    return PAKON_OK;
}

pakon_result pakon_cmd_service_events_dx(pakon_dev *dev, uint8_t low,
                                         uint8_t scn, unsigned timeout_ms,
                                         uint8_t *host_flags, unsigned *serviced,
                                         pakon_dx_roll *dx)
{
    return service_events(dev, low, scn, timeout_ms, host_flags, serviced, dx, NULL);
}

pakon_result pakon_cmd_lamp_temps(pakon_dev *dev, uint8_t low, unsigned timeout_ms,
                                  double *lamp, double *board)
{
    pakon_packet pkt, reply;
    pakon_result r;

    if (!dev)
        return PAKON_ERR_PARAM;
    pakon_lamp_temps_frame(&pkt, low);
    if ((r = pakon_cmd(dev, &pkt, &reply, timeout_ms)) != PAKON_OK)
        return r;
    return pakon_lamp_temps_parse(&reply, low, lamp, board) ? PAKON_OK : PAKON_ERR_PROTO;
}

static unsigned elapsed_ms(const struct timespec *t0)
{
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return (unsigned)((now.tv_sec - t0->tv_sec) * 1000 +
                      (now.tv_nsec - t0->tv_nsec) / 1000000);
}

pakon_result pakon_cmd_wait_lamp(pakon_dev *dev, uint8_t low, uint8_t scn,
                                 unsigned timeout_ms, unsigned limit_ms,
                                 pakon_lamp_progress progress, void *user)
{
    pakon_packet pkt, reply;
    pakon_result r;
    struct timespec t0;
    unsigned next_flags = 0;

    if (!dev)
        return PAKON_ERR_PARAM;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    for (;;) {
        uint8_t status = 0, flags = 0;
        if ((r = service_events(dev, low, scn, timeout_ms, NULL, NULL, NULL,
                                &status)) != PAKON_OK)
            return r;
        if (status & PAKON_LAMP_STABLE) {
            pakon_logf(PAKON_LOG_INFO, "lamp: stable (event status 0x%02x)", status);
            return PAKON_OK;
        }
        unsigned ms = elapsed_ms(&t0);
        if (ms >= next_flags) {          /* the flags and temperatures, once a second */
            next_flags = ms + 1000;
            pakon_lamp_flags_frame(&pkt, low);
            if ((r = pakon_cmd(dev, &pkt, &reply, timeout_ms)) != PAKON_OK)
                return r;
            if (!pakon_lamp_flags_parse(&reply, low, &flags))
                return PAKON_ERR_PROTO;
            if (flags & PAKON_LAMP_FAULT) {
                pakon_logf(PAKON_LOG_ERROR, "lamp: fault (flags 0x%02x)", flags);
                return PAKON_ERR_STATUS;
            }
            if (flags & PAKON_LAMP_STABLE) {
                pakon_logf(PAKON_LOG_INFO, "lamp: stable (flags 0x%02x)", flags);
                return PAKON_OK;
            }
            double lamp = 0, board = 0;
            if (pakon_cmd_lamp_temps(dev, low, timeout_ms, &lamp, &board) == PAKON_OK) {
                pakon_logf(PAKON_LOG_INFO, "lamp: %.2f degC, board %.2f degC, flags 0x%02x",
                           lamp, board, flags);
                if (progress)
                    progress(lamp, ms, user);
            }
        }
        if (ms >= limit_ms)
            return PAKON_ERR_TIMEOUT;
        sleep_ms(200);                   /* the OEM's idle poll */
    }
}
