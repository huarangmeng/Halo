#ifndef HALO_APPLE_PAIRING_H
#define HALO_APPLE_PAIRING_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

int32_t halo_apple_pairing_attach(
    uint64_t session_id,
    const uint8_t *peer_presence_id,
    size_t peer_presence_id_len,
    int32_t role,
    const uint8_t *channel_binding,
    size_t channel_binding_len,
    uint64_t *channel_id_out);

int32_t halo_apple_pairing_submit(
    uint64_t session_id,
    uint64_t channel_id,
    const uint8_t *frame,
    size_t frame_len);

int32_t halo_apple_pairing_drain(
    uint64_t session_id,
    uint64_t channel_id,
    uint8_t *frame_out,
    size_t frame_capacity,
    size_t *frame_len_out);

int32_t halo_apple_pairing_state(uint64_t session_id, uint64_t channel_id);
int32_t halo_apple_pairing_close(uint64_t session_id, uint64_t channel_id);

#ifdef __cplusplus
}
#endif

#endif
