/**
 * @file plane_a_emulator_harness.c
 * @brief Reference Plane A emulator adapter demonstrating shm_open arena snapshots,
 *        libretro-style normalized guest RAM publishing, explicit FRAME_GAP signaling,
 *        and SAVE_STATE_LOAD signaling with zero game-thread blocking.
 *
 * Legacy mode (argc <= 2): runs `shared.emu.sample` frames 1..5 for backwards compatibility.
 * Phase 2 modes:
 *   plane_a_emulator_harness <sock_path> --scenario <cross-runtime|contention|save-load>
 *                              [--mode <standard|encore|spectator>]
 *                              [--frames <N>] [--sleep-us <US>]
 */
#define _POSIX_C_SOURCE 200809L

#include "onehundo/onehundo.h"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static void sleep_micros(unsigned long us) {
    struct timespec ts;
    if (us == 0) {
        return;
    }
    ts.tv_sec = (time_t)(us / 1000000UL);
    ts.tv_nsec = (long)((us % 1000000UL) * 1000UL);
    nanosleep(&ts, NULL);
}

static void write_u16_le(uint8_t *buf, size_t off, uint16_t val) {
    buf[off] = (uint8_t)(val & 0xFFU);
    buf[off + 1U] = (uint8_t)((val >> 8) & 0xFFU);
}

static void write_u32_le(uint8_t *buf, size_t off, uint32_t val) {
    buf[off] = (uint8_t)(val & 0xFFU);
    buf[off + 1U] = (uint8_t)((val >> 8) & 0xFFU);
    buf[off + 2U] = (uint8_t)((val >> 16) & 0xFFU);
    buf[off + 3U] = (uint8_t)((val >> 24) & 0xFFU);
}

static uint32_t crc32_ieee(const uint8_t *data, size_t len) {
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

static int run_legacy_sample(const char *sock) {
    onehundo_client_config_t cfg;
    onehundo_context_t *ctx = NULL;
    onehundo_region_descriptor_t region;
    uint32_t frame;

    onehundo_client_config_init(&cfg);
    cfg.canonical_game_id = "shared.emu.sample";
    cfg.producer_id = "ares_snes_core";
    cfg.session_id = "11111111-1111-4111-8111-111111111111";
    cfg.ingestion_plane = ONEHUNDO_PLANE_MEMORY_ARENA;
    cfg.progression_mode = ONEHUNDO_PROGRESSION_MODE_STANDARD;
    cfg.trust_tier = ONEHUNDO_TRUST_TIER_VERIFIED_INTEGRATION;
    cfg.ipc_endpoint_path = sock;

    if (onehundo_init(&ctx, &cfg, sizeof(cfg)) != ONEHUNDO_OK || !ctx) {
        return 1;
    }

    memset(&region, 0, sizeof(region));
    region.struct_size = sizeof(region);
    region.api_version = ONEHUNDO_API_VERSION;
    region.region_id = 0;
    region.guest_base_address = 0x7E0000ULL;
    region.size_bytes = 4096U;
    region.endianness = ONEHUNDO_ENDIAN_LITTLE;
    region.coherence = ONEHUNDO_COHERENCE_COHERENT;
    region.region_name = "WRAM";
    (void)onehundo_arena_register_region(ctx, &region, sizeof(region));

    for (frame = 1; frame <= 5; ++frame) {
        uint32_t slot_idx = 0;
        void *buf = NULL;
        size_t cap = 0;
        if (onehundo_arena_acquire_slot(ctx, &slot_idx, &buf, &cap) == ONEHUNDO_OK) {
            uint8_t *ram = (uint8_t *)buf;
            onehundo_snapshot_slot_header_t hdr;
            memset(ram, 0, 256);
            ram[0x10] = (uint8_t)frame;
            if (frame >= 4) {
                ram[0x20] = 1; /* Trigger bit/byte condition at 0x20 */
            }
            memset(&hdr, 0, sizeof(hdr));
            hdr.struct_size = sizeof(hdr);
            hdr.api_version = ONEHUNDO_API_VERSION;
            hdr.sequence_number = frame;
            hdr.timestamp_nanos = (uint64_t)frame * 16666667ULL;
            hdr.lifecycle_signal = (frame == 3) ? ONEHUNDO_SIGNAL_SAVE_STATE_LOAD : ONEHUNDO_SIGNAL_NORMAL_FRAME;
            hdr.payload_size_bytes = 256;
            (void)onehundo_arena_publish_slot(ctx, slot_idx, &hdr);
        } else {
            (void)onehundo_emit_lifecycle_signal(ctx, ONEHUNDO_SIGNAL_FRAME_GAP, 1);
        }
        sleep_micros(10000UL);
    }

    onehundo_shutdown(ctx);
    return 0;
}

int main(int argc, char **argv) {
    const char *sock = (argc > 1) ? argv[1] : NULL;
    const char *scenario = NULL;
    uint32_t mode = ONEHUNDO_PROGRESSION_MODE_STANDARD;
    const char *session_id = "22222222-2222-4222-8222-222222222222";
    uint32_t total_frames = 5U;
    int frames_explicit = 0;
    unsigned long frame_sleep_us = 12000UL;
    int i;

    for (i = 2; i < argc; ++i) {
        if (strcmp(argv[i], "--scenario") == 0 && i + 1 < argc) {
            scenario = argv[++i];
        } else if (strcmp(argv[i], "--mode") == 0 && i + 1 < argc) {
            const char *m = argv[++i];
            if (strcmp(m, "spectator") == 0) {
                mode = ONEHUNDO_PROGRESSION_MODE_SPECTATOR;
            } else if (strcmp(m, "encore") == 0) {
                mode = ONEHUNDO_PROGRESSION_MODE_ENCORE;
            } else {
                mode = ONEHUNDO_PROGRESSION_MODE_STANDARD;
            }
        } else if (strcmp(argv[i], "--session") == 0 && i + 1 < argc) {
            session_id = argv[++i];
        } else if (strcmp(argv[i], "--frames") == 0 && i + 1 < argc) {
            long f = strtol(argv[++i], NULL, 10);
            if (f > 0) {
                total_frames = (uint32_t)f;
                frames_explicit = 1;
            }
        } else if (strcmp(argv[i], "--sleep-us") == 0 && i + 1 < argc) {
            long s = strtol(argv[++i], NULL, 10);
            if (s >= 0) {
                frame_sleep_us = (unsigned long)s;
            }
        }
    }

    if (!scenario) {
        return run_legacy_sample(sock);
    }

    {
        onehundo_client_config_t cfg;
        onehundo_context_t *ctx = NULL;
        onehundo_region_descriptor_t region;
        onehundo_diagnostics_t diag;
        uint32_t frame;
        uint32_t pending_gaps = 0U;
        uint32_t contention_mode = (strcmp(scenario, "contention") == 0) ? 1U : 0U;
        uint32_t save_load_mode = (strcmp(scenario, "save-load") == 0) ? 1U : 0U;

        if (contention_mode && !frames_explicit) {
            total_frames = 64U;
            frame_sleep_us = 0UL;
        }
        if (save_load_mode && !frames_explicit && total_frames < 6U) {
            total_frames = 6U;
        }

        onehundo_client_config_init(&cfg);
        cfg.canonical_game_id = "shared.cross_runtime.chronos";
        cfg.producer_id = "libretro_chronos_emu";
        cfg.session_id = session_id;
        cfg.ingestion_plane = ONEHUNDO_PLANE_MEMORY_ARENA;
        cfg.progression_mode = mode;
        cfg.trust_tier = ONEHUNDO_TRUST_TIER_VERIFIED_INTEGRATION;
        cfg.ipc_endpoint_path = sock;

        if (onehundo_init(&ctx, &cfg, sizeof(cfg)) != ONEHUNDO_OK || !ctx) {
            fprintf(stderr, "[plane_a_emulator_harness] onehundo_init failed\n");
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

        /* Allow daemon IPC thread to open the POSIX SHM ring before frame burst */
        sleep_micros(25000UL);

        for (frame = 1; frame <= total_frames; ++frame) {
            uint32_t slot_idx = 0;
            void *buf = NULL;
            size_t cap = 0;
            int32_t rc = onehundo_arena_acquire_slot(ctx, &slot_idx, &buf, &cap);
            if (rc == ONEHUNDO_OK && cap >= 256U) {
                uint8_t *ram = (uint8_t *)buf;
                onehundo_snapshot_slot_header_t hdr;
                memset(ram, 0, 256);

                /* Normalized guest RAM transitions matching onehundo.plane_a.cross_runtime */
                ram[0x04] = 1U; /* in_gameplay */
                if (frame == 1U) {
                    write_u16_le(ram, 0x08U, 100U); /* boss_hp */
                    ram[0x14] = 1U;
                    ram[0x15] = 1U;
                    write_u32_le(ram, 0x18U, 100U);
                } else if (frame == 2U) {
                    write_u16_le(ram, 0x08U, 60U);
                    ram[0x14] = 2U;
                    ram[0x15] = 2U;
                    write_u32_le(ram, 0x18U, 500U);
                    ram[0x20] = 1U;
                    ram[0x28] = 0x01U;
                } else if (frame == 3U) {
                    write_u16_le(ram, 0x08U, 60U);
                    ram[0x0C] = 1U; /* pause_if active -> holds ach_relic_hoard */
                    ram[0x14] = 3U;
                    ram[0x15] = 2U; /* sum == 5 */
                    write_u32_le(ram, 0x18U, 900U);
                    ram[0x20] = 1U; /* 2nd hit -> unlocks ach_phase_flurry */
                    ram[0x28] = 0x01U;
                } else if (frame == 4U || (save_load_mode && frame == 5U)) {
                    /* Frame 4 unpauses ach_relic_hoard; in save-load mode, Frame 5 restores a state with boss_hp=60 */
                    write_u16_le(ram, 0x08U, 60U);
                    ram[0x0C] = 0U;
                    ram[0x14] = 3U;
                    ram[0x15] = 2U;
                    write_u32_le(ram, 0x18U, 1200U);
                } else {
                    write_u16_le(ram, 0x08U, 0U); /* boss_hp 60 -> 0 -> unlocks ach_boss_vanquished */
                    ram[0x14] = 3U;
                    ram[0x15] = 2U;
                    write_u32_le(ram, 0x18U, 2500U);
                    ram[0x24] = 1U; /* bosses_defeated STAT_INC */
                }

                memset(&hdr, 0, sizeof(hdr));
                hdr.struct_size = sizeof(hdr);
                hdr.api_version = ONEHUNDO_API_VERSION;
                hdr.sequence_number = frame;
                hdr.timestamp_nanos = (uint64_t)frame * 16666667ULL;
                if (pending_gaps > 0U) {
                    hdr.lifecycle_signal = ONEHUNDO_SIGNAL_FRAME_GAP;
                    hdr.frame_gap_count = pending_gaps;
                    pending_gaps = 0U;
                } else if (save_load_mode && frame == 5U) {
                    hdr.lifecycle_signal = ONEHUNDO_SIGNAL_SAVE_STATE_LOAD;
                } else {
                    hdr.lifecycle_signal = ONEHUNDO_SIGNAL_NORMAL_FRAME;
                }
                hdr.payload_size_bytes = 256U;
                hdr.crc32 = crc32_ieee(ram, 256U);
                (void)onehundo_arena_publish_slot(ctx, slot_idx, &hdr);
            } else {
                pending_gaps += 1U;
                (void)onehundo_emit_lifecycle_signal(ctx, ONEHUNDO_SIGNAL_FRAME_GAP, 1U);
            }
            sleep_micros(frame_sleep_us);
        }

        if (pending_gaps > 0U) {
            (void)onehundo_emit_lifecycle_signal(ctx, ONEHUNDO_SIGNAL_FRAME_GAP, pending_gaps);
        }

        sleep_micros(30000UL);
        memset(&diag, 0, sizeof(diag));
        diag.struct_size = sizeof(diag);
        (void)onehundo_get_diagnostics(ctx, &diag, sizeof(diag));
        printf("emu snapshots=%llu gaps=%llu connected=%u\n",
               (unsigned long long)diag.frame_snapshots_published,
               (unsigned long long)diag.frame_gaps_recorded,
               (unsigned)diag.is_connected);

        onehundo_shutdown(ctx);
        return 0;
    }
}
