//! Resumable multi-file workflows for authenticated Halo sessions.

#![forbid(unsafe_code)]

mod batch;

pub use halo_transport::{DataIo, DataIoError};

pub use batch::{
    BatchResumeStore, BatchSendJobStore, BatchSource, BatchTransferError, PreparedBatch,
    ReceivedBatch, prepare_batch, prepare_batch_with_id, receive_batch_data_with_progress,
    receive_manifest, resume_positions, send_batch_cancel, send_batch_complete,
    send_batch_data_with_progress, send_batch_decision, send_batch_pause, send_manifest,
    wait_for_batch_complete,
};

use halo_protocol::MAX_FILE_NAME_LEN;
use thiserror::Error;

pub fn validate_file_name(name: &str) -> Result<(), FileNameError> {
    if name.is_empty() || name.len() > MAX_FILE_NAME_LEN || name == "." || name == ".." {
        return Err(FileNameError);
    }
    if name.ends_with([' ', '.'])
        || name.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
    {
        return Err(FileNameError);
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .is_some_and(is_reserved_device_number)
        || stem
            .strip_prefix("LPT")
            .is_some_and(is_reserved_device_number);
    if reserved {
        return Err(FileNameError);
    }
    Ok(())
}

fn is_reserved_device_number(value: &str) -> bool {
    value.len() == 1 && matches!(value.as_bytes()[0], b'1'..=b'9')
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("file name is not a safe cross-platform leaf name")]
pub struct FileNameError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_platform_leaf_names_reject_traversal_and_device_files() {
        for invalid in [
            "",
            ".",
            "..",
            "../escape",
            "folder/file",
            "folder\\file",
            "CON",
            "con.txt",
            "LPT9.log",
            "trailing.",
            "trailing ",
            "bad:name",
            "bad\0name",
        ] {
            assert_eq!(
                validate_file_name(invalid),
                Err(FileNameError),
                "accepted {invalid:?}"
            );
        }
        for valid in [
            "photo.jpg",
            "报告 2026.pdf",
            "archive.tar.gz",
            "hello (1).txt",
        ] {
            assert!(validate_file_name(valid).is_ok(), "rejected {valid:?}");
        }
    }
}
