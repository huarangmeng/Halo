//! Narrow JNI boundary for transferring an Android-bound UDP socket to Rust.

use std::{
    ffi::c_void,
    net::UdpSocket,
    os::fd::{FromRawFd, OwnedFd},
};

use crate::platform_socket::{
    disable_lan_endpoint, register_bound_lan_socket, register_user_approved_hotspot_socket,
};

const STATUS_OK: i32 = 0;
const ERROR_INVALID_ARGUMENT: i32 = -1;
const ERROR_INTERNAL: i32 = -2;

#[unsafe(no_mangle)]
pub unsafe extern "system" fn Java_org_halo_halo_1demo_HaloNativeSocketBridge_nativeRegisterBoundSocket(
    _environment: *mut c_void,
    _receiver: *mut c_void,
    file_descriptor: i32,
) -> i32 {
    if file_descriptor < 0 {
        return ERROR_INVALID_ARGUMENT;
    }
    // SAFETY: Kotlin detached this descriptor from ParcelFileDescriptor and
    // transfers its sole ownership to this function. Constructing OwnedFd
    // immediately ensures every return path either stores or closes it once.
    let owned = unsafe { OwnedFd::from_raw_fd(file_descriptor) };
    let socket = UdpSocket::from(owned);
    match register_bound_lan_socket(socket) {
        Ok(()) => STATUS_OK,
        Err(()) => ERROR_INTERNAL,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn Java_org_halo_halo_1demo_HaloNativeSocketBridge_nativeRegisterUserApprovedHotspotSocket(
    _environment: *mut c_void,
    _receiver: *mut c_void,
    file_descriptor: i32,
) -> i32 {
    if file_descriptor < 0 {
        return ERROR_INVALID_ARGUMENT;
    }
    // SAFETY: Kotlin transfers sole ownership of the detached descriptor under
    // the same contract as nativeRegisterBoundSocket. The distinct entry point
    // attests that a foreground user action selected a local-only hotspot.
    let owned = unsafe { OwnedFd::from_raw_fd(file_descriptor) };
    let socket = UdpSocket::from(owned);
    match register_user_approved_hotspot_socket(socket) {
        Ok(()) => STATUS_OK,
        Err(()) => ERROR_INTERNAL,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_halo_halo_1demo_HaloNativeSocketBridge_nativeDisableLan(
    _environment: *mut c_void,
    _receiver: *mut c_void,
) -> i32 {
    match disable_lan_endpoint() {
        Ok(()) => STATUS_OK,
        Err(()) => ERROR_INTERNAL,
    }
}
