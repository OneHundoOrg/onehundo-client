/**
 * @file bbport_live_demo.c
 * @brief Native source port Plane B integration linking pinned `onehundo-protocol` C99 SDK.
 */
#define _POSIX_C_SOURCE 200809L

#include "onehundo/onehundo.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

int main(int argc, char **argv) {
    const char *sock = (argc > 1) ? argv[1] : NULL;
    onehundo_client_config_t cfg;
    onehundo_context_t *ctx = NULL;
    onehundo_diagnostics_t diag;
    int tries;

    onehundo_client_config_init(&cfg);
    cfg.canonical_game_id = "sony.bloodborne";
    cfg.producer_id = "bbport_pc_x64";
    cfg.session_id = "550e8400-e29b-41d4-a716-446655440000";
    cfg.ingestion_plane = ONEHUNDO_PLANE_SEMANTIC_EVENT;
    cfg.progression_mode = ONEHUNDO_PROGRESSION_MODE_STANDARD;
    cfg.trust_tier = ONEHUNDO_TRUST_TIER_VERIFIED_INTEGRATION;
    cfg.ipc_endpoint_path = sock;

    if (onehundo_init(&ctx, &cfg, sizeof(cfg)) != ONEHUNDO_OK || !ctx) {
        fprintf(stderr, "[bbport_live_demo] init failed\n");
        return 1;
    }

    (void)onehundo_state_observe(ctx, "current_zone", "central_yharnam");
    (void)onehundo_stat_set(ctx, "blood_echoes_held", 4250);
    (void)onehundo_stat_inc(ctx, "bosses_defeated", 1);
    (void)onehundo_unlock_request(ctx, "ach_cleric_beast");

    for (tries = 0; tries < 100; ++tries) {
        memset(&diag, 0, sizeof(diag));
        diag.struct_size = sizeof(diag);
        if (onehundo_get_diagnostics(ctx, &diag, sizeof(diag)) == ONEHUNDO_OK) {
            if (diag.is_connected && diag.events_acked >= 4) {
                break;
            }
            if (!diag.is_connected) {
                break;
            }
        }
        {
            struct timespec ts = {0, 5000000};
            nanosleep(&ts, NULL);
        }
    }

    printf("connected=%u emitted=%llu acked=%llu\n",
           (unsigned)diag.is_connected,
           (unsigned long long)diag.events_emitted,
           (unsigned long long)diag.events_acked);

    onehundo_shutdown(ctx);
    return 0;
}
