/**
 * @file plane_a_emulator_harness.c
 * @brief Reference Plane A emulator adapter demonstrating shm_open arena snapshots,
 *        explicit FRAME_GAP signaling, and SAVE_STATE_LOAD signaling.
 */
#define _POSIX_C_SOURCE 200809L

#include "onehundo/onehundo.h"
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <time.h>

int main(int argc, char **argv) {
    const char *sock = (argc > 1) ? argv[1] : NULL;
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
        {
            struct timespec ts = {0, 10000000};
            nanosleep(&ts, NULL);
        }
    }

    onehundo_shutdown(ctx);
    return 0;
}
