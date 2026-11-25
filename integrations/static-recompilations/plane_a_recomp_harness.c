/**
 * @file plane_a_recomp_harness.c
 * @brief Reference Plane A static-recompilation flat-memory harness proving that
 *        the exact same `.onehundo` memory pack (`onehundo.plane_a.cross_runtime`)
 *        produces identical RUNTIME_OBSERVED unlocks and stats across both classic
 *        emulators and flat-memory static recompilations without blocking the game loop.
 */
#define _POSIX_C_SOURCE 200809L

#include "onehundo/onehundo.h"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* Flat deterministic guest RAM buffer exposed by a static recompilation runtime */
static uint8_t g_recomp_guest_ram[4096];

static void sleep_micros(unsigned long us) {
    struct timespec ts;
    if (us == 0) {
        return;
    }
    ts.tv_sec = (time_t)(us / 1000000UL);
    ts.tv_nsec = (long)((us % 1000000UL) * 1000UL);
    nanosleep(&ts, NULL);
}

static void recomp_write_u16_le(size_t off, uint16_t val) {
    g_recomp_guest_ram[off] = (uint8_t)(val & 0xFFU);
    g_recomp_guest_ram[off + 1U] = (uint8_t)((val >> 8) & 0xFFU);
}

static void recomp_write_u32_le(size_t off, uint32_t val) {
    g_recomp_guest_ram[off] = (uint8_t)(val & 0xFFU);
    g_recomp_guest_ram[off + 1U] = (uint8_t)((val >> 8) & 0xFFU);
    g_recomp_guest_ram[off + 2U] = (uint8_t)((val >> 16) & 0xFFU);
    g_recomp_guest_ram[off + 3U] = (uint8_t)((val >> 24) & 0xFFU);
}

static uint32_t recomp_crc32_ieee(const uint8_t *data, size_t len) {
    uint32_t crc = 0xFFFFFFFFU;
    size_t i;
    int bit;
    for (i = 0; i < len; ++i) {
        crc ^= (uint32_t)data[i];
        for (bit = 0; bit < 8; ++bit) {
            uint32_t mask = (crc & 1U) ? 0xEDB88320U : 0U;
            crc = (crc >> 1) ^ mask;
        }
    }
    return ~crc;
}

static void recomp_step_gameplay_frame(uint32_t frame, int save_load_mode) {
    g_recomp_guest_ram[0x04] = 1U; /* in_gameplay */
    if (frame == 1U) {
        recomp_write_u16_le(0x08U, 100U);
        g_recomp_guest_ram[0x14] = 1U;
        g_recomp_guest_ram[0x15] = 1U;
        recomp_write_u32_le(0x18U, 100U);
    } else if (frame == 2U) {
        recomp_write_u16_le(0x08U, 60U);
        g_recomp_guest_ram[0x14] = 2U;
        g_recomp_guest_ram[0x15] = 2U;
        recomp_write_u32_le(0x18U, 500U);
        g_recomp_guest_ram[0x20] = 1U;
        g_recomp_guest_ram[0x28] = 0x02U; /* Alt group bit1 (0xN28=1) */
    } else if (frame == 3U) {
        g_recomp_guest_ram[0x0C] = 1U; /* pause_guard */
        g_recomp_guest_ram[0x14] = 3U;
        g_recomp_guest_ram[0x15] = 2U;
        recomp_write_u32_le(0x18U, 900U);
        g_recomp_guest_ram[0x20] = 1U;
        g_recomp_guest_ram[0x28] = 0x02U;
    } else if (frame == 4U || (save_load_mode && frame == 5U)) {
        g_recomp_guest_ram[0x0C] = 0U; /* pause cleared -> relic_hoard */
        recomp_write_u16_le(0x08U, 60U);
        recomp_write_u32_le(0x18U, 1200U);
    } else {
        recomp_write_u16_le(0x08U, 0U); /* boss defeated -> ach_boss_vanquished */
        recomp_write_u32_le(0x18U, 2500U);
        g_recomp_guest_ram[0x24] = 1U;
    }
}

int main(int argc, char **argv) {
    const char *sock = (argc > 1) ? argv[1] : NULL;
    uint32_t mode = ONEHUNDO_PROGRESSION_MODE_STANDARD;
    const char *session_id = "33333333-3333-4333-8333-333333333333";
    int save_load_mode = 0;
    uint32_t total_frames = 5U;
    int i;

    onehundo_client_config_t cfg;
    onehundo_context_t *ctx = NULL;
    onehundo_region_descriptor_t region;
    onehundo_diagnostics_t diag;
    uint32_t frame;

    for (i = 2; i < argc; ++i) {
        if (strcmp(argv[i], "--mode") == 0 && i + 1 < argc) {
            const char *m = argv[++i];
            if (strcmp(m, "spectator") == 0) {
                mode = ONEHUNDO_PROGRESSION_MODE_SPECTATOR;
            } else if (strcmp(m, "encore") == 0) {
                mode = ONEHUNDO_PROGRESSION_MODE_ENCORE;
            }
        } else if (strcmp(argv[i], "--session") == 0 && i + 1 < argc) {
            session_id = argv[++i];
        } else if (strcmp(argv[i], "--save-load") == 0) {
            save_load_mode = 1;
            total_frames = 6U;
        }
    }

    memset(g_recomp_guest_ram, 0, sizeof(g_recomp_guest_ram));
    onehundo_client_config_init(&cfg);
    cfg.canonical_game_id = "shared.cross_runtime.chronos";
    cfg.producer_id = "chronos_static_recomp_x64";
    cfg.session_id = session_id;
    cfg.ingestion_plane = ONEHUNDO_PLANE_MEMORY_ARENA;
    cfg.progression_mode = mode;
    cfg.trust_tier = ONEHUNDO_TRUST_TIER_VERIFIED_INTEGRATION;
    cfg.ipc_endpoint_path = sock;

    if (onehundo_init(&ctx, &cfg, sizeof(cfg)) != ONEHUNDO_OK || !ctx) {
        fprintf(stderr, "[plane_a_recomp_harness] onehundo_init failed\n");
        return 1;
    }

    memset(&region, 0, sizeof(region));
    region.struct_size = sizeof(region);
    region.api_version = ONEHUNDO_API_VERSION;
    region.region_id = 0;
    region.guest_base_address = 0x00000000ULL;
    region.size_bytes = 4096U;
    region.endianness = ONEHUNDO_ENDIAN_LITTLE;
    region.coherence = ONEHUNDO_COHERENCE_COHERENT;
    region.region_name = "Main RAM";
    (void)onehundo_arena_register_region(ctx, &region, sizeof(region));

    sleep_micros(25000UL);

    for (frame = 1U; frame <= total_frames; ++frame) {
        uint32_t slot_idx = 0U;
        void *buf = NULL;
        size_t cap = 0U;
        recomp_step_gameplay_frame(frame, save_load_mode);

        if (onehundo_arena_acquire_slot(ctx, &slot_idx, &buf, &cap) == ONEHUNDO_OK && cap >= 256U) {
            onehundo_snapshot_slot_header_t hdr;
            memcpy(buf, g_recomp_guest_ram, 256U);
            memset(&hdr, 0, sizeof(hdr));
            hdr.struct_size = sizeof(hdr);
            hdr.api_version = ONEHUNDO_API_VERSION;
            hdr.sequence_number = frame;
            hdr.timestamp_nanos = (uint64_t)frame * 16666667ULL;
            hdr.lifecycle_signal = (save_load_mode && frame == 5U)
                                       ? ONEHUNDO_SIGNAL_SAVE_STATE_LOAD
                                       : ONEHUNDO_SIGNAL_NORMAL_FRAME;
            hdr.payload_size_bytes = 256U;
            hdr.crc32 = recomp_crc32_ieee((const uint8_t *)buf, 256U);
            (void)onehundo_arena_publish_slot(ctx, slot_idx, &hdr);
        } else {
            (void)onehundo_emit_lifecycle_signal(ctx, ONEHUNDO_SIGNAL_FRAME_GAP, 1U);
        }
        sleep_micros(12000UL);
    }

    sleep_micros(30000UL);
    memset(&diag, 0, sizeof(diag));
    diag.struct_size = sizeof(diag);
    (void)onehundo_get_diagnostics(ctx, &diag, sizeof(diag));
    printf("recomp snapshots=%llu gaps=%llu connected=%u\n",
           (unsigned long long)diag.frame_snapshots_published,
           (unsigned long long)diag.frame_gaps_recorded,
           (unsigned)diag.is_connected);

    onehundo_shutdown(ctx);
    return 0;
}
