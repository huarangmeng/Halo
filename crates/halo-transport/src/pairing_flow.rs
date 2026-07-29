use async_trait::async_trait;
use thiserror::Error;

use halo_crypto::{
    DeviceIdentity, PairingCode, PairingCryptoError, PeerId, StoreError, TrustStore, TrustedPeer,
    create_client_hello, create_commit, create_decision, create_server_hello, derive_peer_id,
    pairing_code, transcript_hash, verify_client_hello, verify_commit, verify_decision,
    verify_server_hello,
};
use halo_protocol::{Capabilities, PairingMessage, ProtocolRange};

use crate::{ControlIo, FrameIoError, receive_message, send_message};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PairingPrompt {
    pub peer_id: PeerId,
    pub code: PairingCode,
    pub confirmation_required: bool,
}

/// Flutter implements this narrow UI boundary. Rust decides when confirmation
/// is required and ignores UI attempts to bypass a required rejection.
#[async_trait]
pub trait PairingUserInteraction: Send + Sync {
    async fn present(&self, prompt: PairingPrompt) -> Result<bool, PairingFlowError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairingOutcome {
    pub peer: TrustedPeer,
    pub already_trusted: bool,
}

pub async fn pair_as_initiator(
    io: &mut dyn ControlIo,
    identity: &DeviceIdentity,
    versions: ProtocolRange,
    capabilities: Capabilities,
    expected_peer: Option<&TrustedPeer>,
    trust_store: &dyn TrustStore,
    interaction: &dyn PairingUserInteraction,
) -> Result<PairingOutcome, PairingFlowError> {
    let binding = io.channel_binding();
    let client = create_client_hello(identity, versions, capabilities, binding)?;
    let client_frame = PairingMessage::ClientHello(client.clone()).encode();
    send_message(io, &PairingMessage::ClientHello(client.clone())).await?;

    let server = match receive_message(io).await? {
        PairingMessage::ServerHello(message) => message,
        _ => return Err(PairingFlowError::UnexpectedMessage),
    };
    let server_frame = PairingMessage::ServerHello(server.clone()).encode();
    let server_key = verify_server_hello(&client_frame, &client, &server, binding)?;
    let (peer, already_trusted) = recognize_peer(
        server_key,
        server.selected_version,
        expected_peer,
        trust_store,
    )
    .await?;
    let transcript = transcript_hash(binding, &client_frame, &server_frame);
    if !already_trusted {
        let displayed = interaction
            .present(PairingPrompt {
                peer_id: peer.peer_id(),
                code: pairing_code(&transcript)?,
                confirmation_required: false,
            })
            .await?;
        if !displayed {
            return Err(PairingFlowError::UserCancelled);
        }
    }

    let decision = match receive_message(io).await? {
        PairingMessage::Decision(message) => message,
        _ => return Err(PairingFlowError::UnexpectedMessage),
    };
    verify_decision(&peer.identity_key, &decision, &transcript, binding)?;
    let commit = create_commit(identity, transcript, binding);
    send_message(io, &PairingMessage::Commit(commit)).await?;
    trust_store.save(&peer).await?;
    Ok(PairingOutcome {
        peer,
        already_trusted,
    })
}

pub async fn pair_as_responder(
    io: &mut dyn ControlIo,
    identity: &DeviceIdentity,
    versions: ProtocolRange,
    capabilities: Capabilities,
    expected_peer: Option<&TrustedPeer>,
    trust_store: &dyn TrustStore,
    interaction: &dyn PairingUserInteraction,
) -> Result<PairingOutcome, PairingFlowError> {
    let binding = io.channel_binding();
    let client = match receive_message(io).await? {
        PairingMessage::ClientHello(message) => message,
        _ => return Err(PairingFlowError::UnexpectedMessage),
    };
    let client_frame = PairingMessage::ClientHello(client.clone()).encode();
    let client_key = verify_client_hello(&client, binding)?;
    let server = create_server_hello(
        identity,
        versions,
        capabilities,
        &client_frame,
        &client,
        binding,
    )?;
    let server_frame = PairingMessage::ServerHello(server.clone()).encode();
    send_message(io, &PairingMessage::ServerHello(server.clone())).await?;
    let (peer, already_trusted) = recognize_peer(
        client_key,
        server.selected_version,
        expected_peer,
        trust_store,
    )
    .await?;
    let transcript = transcript_hash(binding, &client_frame, &server_frame);
    let accepted = if already_trusted {
        true
    } else {
        interaction
            .present(PairingPrompt {
                peer_id: peer.peer_id(),
                code: pairing_code(&transcript)?,
                confirmation_required: true,
            })
            .await?
    };
    let decision = create_decision(identity, transcript, accepted, binding);
    send_message(io, &PairingMessage::Decision(decision)).await?;
    if !accepted {
        return Err(PairingFlowError::Rejected);
    }

    let commit = match receive_message(io).await? {
        PairingMessage::Commit(message) => message,
        _ => return Err(PairingFlowError::UnexpectedMessage),
    };
    verify_commit(&peer.identity_key, &commit, &transcript, binding)?;
    trust_store.save(&peer).await?;
    Ok(PairingOutcome {
        peer,
        already_trusted,
    })
}

async fn recognize_peer(
    identity_key: halo_crypto::IdentityPublicKey,
    protocol_version: u16,
    expected_peer: Option<&TrustedPeer>,
    trust_store: &dyn TrustStore,
) -> Result<(TrustedPeer, bool), PairingFlowError> {
    if let Some(expected) = expected_peer
        && expected.identity_key != identity_key
    {
        return Err(PairingFlowError::IdentityChanged);
    }
    let peer = TrustedPeer {
        identity_key,
        protocol_version,
    };
    let stored = trust_store.load(derive_peer_id(&peer.identity_key)).await?;
    match stored {
        Some(stored) if stored.identity_key == peer.identity_key => Ok((peer, true)),
        Some(_) => Err(PairingFlowError::IdentityChanged),
        None if expected_peer.is_some() => Err(PairingFlowError::IdentityChanged),
        None => Ok((peer, false)),
    }
}

#[derive(Debug, Error)]
pub enum PairingFlowError {
    #[error("pairing control I/O failed: {0}")]
    Io(#[from] FrameIoError),
    #[error("pairing authentication failed: {0}")]
    Crypto(#[from] PairingCryptoError),
    #[error("pairing persistence failed: {0}")]
    Store(#[from] StoreError),
    #[error("peer sent a message that is invalid in the current pairing state")]
    UnexpectedMessage,
    #[error("remembered peer identity changed")]
    IdentityChanged,
    #[error("receiver rejected pairing")]
    Rejected,
    #[error("pairing display or initiating user was cancelled")]
    UserCancelled,
    #[error("pairing UI boundary failed")]
    UserInterface,
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use super::*;

    struct MemoryTrustStore(Mutex<HashMap<PeerId, TrustedPeer>>);

    #[async_trait]
    impl TrustStore for MemoryTrustStore {
        async fn load(&self, peer_id: PeerId) -> Result<Option<TrustedPeer>, StoreError> {
            Ok(self
                .0
                .lock()
                .map_err(|_| StoreError::Persistence)?
                .get(&peer_id)
                .cloned())
        }

        async fn save(&self, peer: &TrustedPeer) -> Result<(), StoreError> {
            self.0
                .lock()
                .map_err(|_| StoreError::Persistence)?
                .insert(peer.peer_id(), peer.clone());
            Ok(())
        }

        async fn delete(&self, peer_id: PeerId) -> Result<(), StoreError> {
            self.0
                .lock()
                .map_err(|_| StoreError::Persistence)?
                .remove(&peer_id);
            Ok(())
        }
    }

    fn empty_store() -> MemoryTrustStore {
        MemoryTrustStore(Mutex::new(HashMap::new()))
    }

    #[tokio::test]
    async fn known_identity_mismatch_is_a_hard_error() {
        let expected_identity =
            DeviceIdentity::generate().unwrap_or_else(|error| panic!("expected identity: {error}"));
        let presented_identity = DeviceIdentity::generate()
            .unwrap_or_else(|error| panic!("presented identity: {error}"));
        let expected = TrustedPeer {
            identity_key: expected_identity.public_key(),
            protocol_version: 1,
        };
        let result = recognize_peer(
            presented_identity.public_key(),
            1,
            Some(&expected),
            &empty_store(),
        )
        .await;
        assert!(matches!(result, Err(PairingFlowError::IdentityChanged)));
    }
}
