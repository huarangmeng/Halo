use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use halo_crypto::{DeviceIdentity, PeerId, StoreError, TrustStore, TrustedPeer};
use halo_protocol::{Capabilities, ProtocolRange};
use halo_transport::{
    PairingFlowError, PairingOutcome, PairingPrompt, PairingUserInteraction, QuicEndpoint,
    SecureConnector, pair_as_initiator, pair_as_responder,
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
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

struct RecordingInteraction {
    accepted: bool,
    prompts: Mutex<Vec<PairingPrompt>>,
}

impl RecordingInteraction {
    fn accepting() -> Self {
        Self {
            accepted: true,
            prompts: Mutex::new(Vec::new()),
        }
    }

    fn prompts(&self) -> Vec<PairingPrompt> {
        self.prompts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[async_trait]
impl PairingUserInteraction for RecordingInteraction {
    async fn present(&self, prompt: PairingPrompt) -> Result<bool, PairingFlowError> {
        self.prompts
            .lock()
            .map_err(|_| PairingFlowError::UserInterface)?
            .push(prompt);
        Ok(self.accepted)
    }
}

async fn pair_once(
    client_identity: Arc<DeviceIdentity>,
    server_identity: Arc<DeviceIdentity>,
    client_store: Arc<MemoryTrustStore>,
    server_store: Arc<MemoryTrustStore>,
    client_ui: Arc<RecordingInteraction>,
    server_ui: Arc<RecordingInteraction>,
) -> (PairingOutcome, PairingOutcome) {
    let server = Arc::new(
        QuicEndpoint::server(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .unwrap_or_else(|error| panic!("server endpoint: {error}")),
    );
    let server_address = server
        .local_addr()
        .unwrap_or_else(|error| panic!("server address: {error}"));
    let server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            let connection = server
                .accept(CancellationToken::new())
                .await
                .unwrap_or_else(|error| panic!("accept: {error}"));
            let mut io = connection
                .accept_control()
                .await
                .unwrap_or_else(|error| panic!("accept control: {error}"));
            pair_as_responder(
                &mut io,
                &server_identity,
                ProtocolRange::new(1, 1).unwrap_or_else(|error| panic!("server range: {error}")),
                Capabilities::from_bits(0b11),
                None,
                server_store.as_ref(),
                server_ui.as_ref(),
            )
            .await
            .unwrap_or_else(|error| panic!("server pairing: {error}"))
        })
    };

    let client = QuicEndpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .unwrap_or_else(|error| panic!("client endpoint: {error}"));
    let connection = client
        .connect(server_address, CancellationToken::new())
        .await
        .unwrap_or_else(|error| panic!("connect: {error}"));
    let mut io = connection
        .open_control()
        .await
        .unwrap_or_else(|error| panic!("open control: {error}"));
    let client_outcome = pair_as_initiator(
        &mut io,
        &client_identity,
        ProtocolRange::new(1, 1).unwrap_or_else(|error| panic!("client range: {error}")),
        Capabilities::from_bits(0b01),
        None,
        client_store.as_ref(),
        client_ui.as_ref(),
    )
    .await
    .unwrap_or_else(|error| panic!("client pairing: {error}"));
    let server_outcome = server_task
        .await
        .unwrap_or_else(|error| panic!("join server: {error}"));
    (client_outcome, server_outcome)
}

#[tokio::test]
async fn first_contact_codes_match_and_restart_recognizes_both_identities() {
    let client_identity = Arc::new(
        DeviceIdentity::generate().unwrap_or_else(|error| panic!("client identity: {error}")),
    );
    let server_identity = Arc::new(
        DeviceIdentity::generate().unwrap_or_else(|error| panic!("server identity: {error}")),
    );
    let client_store = Arc::new(MemoryTrustStore::default());
    let server_store = Arc::new(MemoryTrustStore::default());
    let client_ui = Arc::new(RecordingInteraction::accepting());
    let server_ui = Arc::new(RecordingInteraction::accepting());

    let (client_first, server_first) = pair_once(
        Arc::clone(&client_identity),
        Arc::clone(&server_identity),
        Arc::clone(&client_store),
        Arc::clone(&server_store),
        Arc::clone(&client_ui),
        Arc::clone(&server_ui),
    )
    .await;
    assert!(!client_first.already_trusted);
    assert!(!server_first.already_trusted);
    let client_prompts = client_ui.prompts();
    let server_prompts = server_ui.prompts();
    assert_eq!(client_prompts.len(), 1);
    assert_eq!(server_prompts.len(), 1);
    assert_eq!(client_prompts[0].code, server_prompts[0].code);
    assert!(!client_prompts[0].confirmation_required);
    assert!(server_prompts[0].confirmation_required);

    let (client_second, server_second) = pair_once(
        client_identity,
        server_identity,
        client_store,
        server_store,
        Arc::clone(&client_ui),
        Arc::clone(&server_ui),
    )
    .await;
    assert!(client_second.already_trusted);
    assert!(server_second.already_trusted);
    assert_eq!(client_ui.prompts().len(), 1);
    assert_eq!(server_ui.prompts().len(), 1);
}
