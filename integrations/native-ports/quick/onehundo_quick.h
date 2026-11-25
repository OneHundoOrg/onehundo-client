/**
 * @file onehundo_quick.h
 * @brief Frictionless, fail-open LOCAL lane helper for quick/vibe-coded native ports.
 *
 * Wraps the authoritative `onehundo-protocol` C99 API (`onehundo/onehundo.h`)
 * with zero game-thread blocking, automatic LOCAL trust tier, and fail-open
 * resilience when `onehundod` is offline.
 */
#ifndef ONEHUNDO_QUICK_H
#define ONEHUNDO_QUICK_H

#include "onehundo/onehundo.h"
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

static inline onehundo_context_t *onehundo_quick_init(
    const char *canonical_game_id,
    const char *producer_id,
    const char *ipc_endpoint_or_null
) {
    onehundo_client_config_t cfg;
    onehundo_context_t *ctx = NULL;
    onehundo_client_config_init(&cfg);
    cfg.canonical_game_id = canonical_game_id;
    cfg.producer_id = producer_id;
    cfg.ingestion_plane = ONEHUNDO_PLANE_SEMANTIC_EVENT;
    cfg.progression_mode = ONEHUNDO_PROGRESSION_MODE_STANDARD;
    cfg.trust_tier = ONEHUNDO_TRUST_TIER_LOCAL;
    cfg.ipc_endpoint_path = ipc_endpoint_or_null;
    if (onehundo_init(&ctx, &cfg, sizeof(cfg)) != ONEHUNDO_OK) {
        return NULL;
    }
    return ctx;
}

static inline void onehundo_quick_stat_inc(onehundo_context_t *ctx, const char *stat_key, int64_t delta) {
    if (ctx) {
        (void)onehundo_stat_inc(ctx, stat_key, delta);
    }
}

static inline void onehundo_quick_stat_set(onehundo_context_t *ctx, const char *stat_key, int64_t value) {
    if (ctx) {
        (void)onehundo_stat_set(ctx, stat_key, value);
    }
}

static inline void onehundo_quick_unlock(onehundo_context_t *ctx, const char *achievement_id) {
    if (ctx) {
        (void)onehundo_unlock_request(ctx, achievement_id);
    }
}

static inline void onehundo_quick_shutdown(onehundo_context_t *ctx) {
    if (ctx) {
        onehundo_shutdown(ctx);
    }
}

#ifdef __cplusplus
}
#endif

#endif /* ONEHUNDO_QUICK_H */
