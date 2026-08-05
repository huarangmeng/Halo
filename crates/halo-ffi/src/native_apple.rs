//! Narrow C ABI used by the Apple Network.framework QUIC adapter.
//!
//! Control frames move directly between Swift and the Rust pairing state
//! machine. Dart never sees TLS exporter material or protocol bytes.

use std::{ptr, slice, str};

use crate::{
    HaloApiError, PlatformPairingChannelState, PlatformPairingRole,
    pairing_attach_platform_channel, pairing_close_platform_channel, pairing_drain_platform_frames,
    pairing_platform_channel_state, pairing_submit_platform_frame,
};

const STATUS_OK: i32 = 0;
const STATUS_EMPTY: i32 = 1;
const STATUS_BACKPRESSURE: i32 = 2;
const ERROR_INVALID_ARGUMENT: i32 = -1;
const ERROR_NOT_FOUND: i32 = -2;
const ERROR_INTERNAL: i32 = -3;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn halo_apple_pairing_attach(
    session_id: u64,
    peer_presence_id: *const u8,
    peer_presence_id_len: usize,
    role: i32,
    channel_binding: *const u8,
    channel_binding_len: usize,
    channel_id_out: *mut u64,
) -> i32 {
    if session_id == 0 || channel_id_out.is_null() {
        return ERROR_INVALID_ARGUMENT;
    }
    let role = match role {
        0 => PlatformPairingRole::Initiator,
        1 => PlatformPairingRole::Responder,
        _ => return ERROR_INVALID_ARGUMENT,
    };
    let peer_presence_id = match optional_utf8(peer_presence_id, peer_presence_id_len) {
        Ok(value) => value,
        Err(status) => return status,
    };
    let binding = match borrowed_bytes(channel_binding, channel_binding_len) {
        Ok(value) if value.len() == 32 => value.to_vec(),
        Ok(_) => return ERROR_INVALID_ARGUMENT,
        Err(status) => return status,
    };
    match pairing_attach_platform_channel(session_id, peer_presence_id, role, binding) {
        Ok(channel_id) => {
            // SAFETY: The caller supplied a non-null writable pointer and the
            // function writes exactly one initialized `u64` without retaining it.
            unsafe { ptr::write(channel_id_out, channel_id) };
            STATUS_OK
        }
        Err(error) => error_status(&error),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn halo_apple_pairing_submit(
    session_id: u64,
    channel_id: u64,
    frame: *const u8,
    frame_len: usize,
) -> i32 {
    let frame = match borrowed_bytes(frame, frame_len) {
        Ok(value) => value.to_vec(),
        Err(status) => return status,
    };
    match pairing_submit_platform_frame(session_id, channel_id, frame) {
        Ok(()) => STATUS_OK,
        Err(HaloApiError::Core { message }) if message.contains("backpressured") => {
            STATUS_BACKPRESSURE
        }
        Err(error) => error_status(&error),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn halo_apple_pairing_drain(
    session_id: u64,
    channel_id: u64,
    frame_out: *mut u8,
    frame_capacity: usize,
    frame_len_out: *mut usize,
) -> i32 {
    if frame_out.is_null() || frame_len_out.is_null() || frame_capacity == 0 {
        return ERROR_INVALID_ARGUMENT;
    }
    let mut frames = match pairing_drain_platform_frames(session_id, channel_id, 1) {
        Ok(frames) => frames,
        Err(error) => return error_status(&error),
    };
    let Some(frame) = frames.pop() else {
        // SAFETY: The caller supplied a non-null writable pointer and the
        // function writes exactly one initialized `usize`.
        unsafe { ptr::write(frame_len_out, 0) };
        return STATUS_EMPTY;
    };
    if frame.len() > frame_capacity {
        return ERROR_INVALID_ARGUMENT;
    }
    // SAFETY: Both pointers are non-null, `frame_out` has at least
    // `frame_capacity` writable bytes by contract, and the regions cannot
    // overlap because `frame` is owned by this function.
    unsafe {
        ptr::copy_nonoverlapping(frame.as_ptr(), frame_out, frame.len());
        ptr::write(frame_len_out, frame.len());
    }
    STATUS_OK
}

#[unsafe(no_mangle)]
pub extern "C" fn halo_apple_pairing_state(session_id: u64, channel_id: u64) -> i32 {
    match pairing_platform_channel_state(session_id, channel_id) {
        Ok(PlatformPairingChannelState::Pending) => 0,
        Ok(PlatformPairingChannelState::Authenticated) => 1,
        Ok(PlatformPairingChannelState::Failed) => 2,
        Err(error) => error_status(&error),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn halo_apple_pairing_close(session_id: u64, channel_id: u64) -> i32 {
    match pairing_close_platform_channel(session_id, channel_id) {
        Ok(()) => STATUS_OK,
        Err(error) => error_status(&error),
    }
}

fn optional_utf8(pointer: *const u8, length: usize) -> Result<Option<String>, i32> {
    if length == 0 {
        return Ok(None);
    }
    let bytes = borrowed_bytes(pointer, length)?;
    str::from_utf8(bytes)
        .map(|value| Some(value.to_owned()))
        .map_err(|_| ERROR_INVALID_ARGUMENT)
}

fn borrowed_bytes<'a>(pointer: *const u8, length: usize) -> Result<&'a [u8], i32> {
    if pointer.is_null() || length == 0 {
        return Err(ERROR_INVALID_ARGUMENT);
    }
    // SAFETY: The C caller guarantees `pointer` references `length` readable
    // bytes for this call. The returned slice is consumed before the FFI call
    // returns and is never retained.
    Ok(unsafe { slice::from_raw_parts(pointer, length) })
}

fn error_status(error: &HaloApiError) -> i32 {
    match error {
        HaloApiError::InvalidArgument { .. } => ERROR_INVALID_ARGUMENT,
        HaloApiError::SessionNotFound => ERROR_NOT_FOUND,
        HaloApiError::Core { .. } | HaloApiError::InternalState => ERROR_INTERNAL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_bridge_rejects_null_and_invalid_lengths() {
        let mut channel_id = 0;
        // SAFETY: Deliberately null pointers exercise validation before any
        // dereference; `channel_id` is a valid writable output.
        assert_eq!(
            unsafe {
                halo_apple_pairing_attach(1, ptr::null(), 0, 0, ptr::null(), 32, &mut channel_id)
            },
            ERROR_INVALID_ARGUMENT
        );
        // SAFETY: Deliberately null pointers exercise validation only.
        assert_eq!(
            unsafe { halo_apple_pairing_submit(1, 1, ptr::null(), 12) },
            ERROR_INVALID_ARGUMENT
        );
    }

    #[test]
    fn native_bridge_validates_inputs_before_session_lookup() {
        let peer = b"apple-peer";
        let binding = [0x5a; 32];
        let mut channel_id = 0;
        // SAFETY: All input slices and the output pointer remain valid for the
        // duration of this synchronous call.
        assert_eq!(
            unsafe {
                halo_apple_pairing_attach(
                    u64::MAX,
                    peer.as_ptr(),
                    peer.len(),
                    0,
                    binding.as_ptr(),
                    binding.len(),
                    &mut channel_id,
                )
            },
            ERROR_NOT_FOUND
        );
        assert_eq!(channel_id, 0);
    }
}
