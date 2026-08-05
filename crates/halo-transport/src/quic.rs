use std::{
    fmt,
    net::{SocketAddr, UdpSocket},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use p256::{SecretKey, pkcs8::DecodePrivateKey};
use quinn::{
    ClientConfig, Connection, Endpoint, EndpointConfig, RecvStream, SendStream, ServerConfig,
    TokioRuntime, TransportConfig,
    crypto::rustls::{QuicClientConfig, QuicServerConfig},
};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use halo_crypto::TlsChannelBinding;
use halo_protocol::{DATA_RECORD_HEADER_LEN, HEADER_LEN, MAX_DATA_RECORD_LEN, MAX_FRAME_LEN};

use crate::{
    ConnectAttemptError, ConnectErrorKind, ControlIo, DataIo, DataIoError, FrameIoError,
    SecureConnector, data_io::data_record_length,
};

const ALPN: &[u8] = b"halo-pairing/1";
const EXPORTER_LABEL: &[u8] = b"EXPORTER-Halo-Pairing-v1";
const MAX_CERTIFICATE_LEN: usize = 16 * 1024;

pub struct NativeTlsIdentity {
    pub certificate_der: Vec<u8>,
    /// Apple Security framework's P-256 private external representation:
    /// uncompressed public point followed by the 32-byte secret scalar.
    pub private_key_x963: Vec<u8>,
}

pub fn generate_native_tls_identity() -> Result<NativeTlsIdentity, QuicEndpointError> {
    let certificate = rcgen::generate_simple_self_signed(vec!["halo.invalid".to_owned()])
        .map_err(|_| QuicEndpointError::Certificate)?;
    let pkcs8 = certificate.signing_key.serialize_der();
    let secret = SecretKey::from_pkcs8_der(&pkcs8).map_err(|_| QuicEndpointError::Certificate)?;
    let public = secret.public_key().to_sec1_bytes();
    let mut private_key_x963 = Vec::with_capacity(public.len() + 32);
    private_key_x963.extend_from_slice(&public);
    private_key_x963.extend_from_slice(&secret.to_bytes());
    let certificate_der = certificate.cert.der().to_vec();
    if certificate_der.is_empty()
        || certificate_der.len() > MAX_CERTIFICATE_LEN
        || private_key_x963.len() != 97
    {
        return Err(QuicEndpointError::Certificate);
    }
    Ok(NativeTlsIdentity {
        certificate_der,
        private_key_x963,
    })
}

pub struct QuicEndpoint {
    endpoint: Endpoint,
}

impl QuicEndpoint {
    pub fn client(bind_address: SocketAddr) -> Result<Self, QuicEndpointError> {
        let mut endpoint = Endpoint::client(bind_address).map_err(|_| QuicEndpointError::Bind)?;
        endpoint.set_default_client_config(client_config()?);
        Ok(Self { endpoint })
    }

    pub fn server(bind_address: SocketAddr) -> Result<Self, QuicEndpointError> {
        let mut endpoint = Endpoint::server(server_config()?, bind_address)
            .map_err(|_| QuicEndpointError::Bind)?;
        endpoint.set_default_client_config(client_config()?);
        Ok(Self { endpoint })
    }

    /// Creates a client endpoint from a socket selected and bound by a platform adapter.
    ///
    /// Android `Network`, Apple peer-to-peer, Wi-Fi Direct, and Wi-Fi Aware adapters must
    /// perform their OS-specific network selection before handing ownership to this method.
    /// Halo does not infer an eligible interface from a wildcard-bound socket.
    pub fn client_with_socket(socket: UdpSocket) -> Result<Self, QuicEndpointError> {
        let mut endpoint = endpoint_with_socket(socket, None)?;
        endpoint.set_default_client_config(client_config()?);
        Ok(Self { endpoint })
    }

    /// Creates a bidirectional endpoint from a socket selected and bound by a platform adapter.
    ///
    /// The caller remains responsible for binding the socket to the approved local bearer before
    /// transferring ownership. Active QUIC address migration is disabled by the server policy.
    pub fn server_with_socket(socket: UdpSocket) -> Result<Self, QuicEndpointError> {
        let mut endpoint = endpoint_with_socket(socket, Some(server_config()?))?;
        endpoint.set_default_client_config(client_config()?);
        Ok(Self { endpoint })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, QuicEndpointError> {
        self.endpoint
            .local_addr()
            .map_err(|_| QuicEndpointError::LocalAddress)
    }

    pub async fn accept(
        &self,
        cancellation: CancellationToken,
    ) -> Result<QuicConnection, ConnectAttemptError> {
        let incoming = tokio::select! {
            () = cancellation.cancelled() => {
                return Err(ConnectAttemptError::new(ConnectErrorKind::Cancelled));
            }
            incoming = self.endpoint.accept() => incoming.ok_or_else(|| {
                ConnectAttemptError::new(ConnectErrorKind::Unreachable)
            })?,
        };
        let connection = tokio::select! {
            () = cancellation.cancelled() => {
                return Err(ConnectAttemptError::new(ConnectErrorKind::Cancelled));
            }
            result = incoming => result.map_err(|_| {
                ConnectAttemptError::new(ConnectErrorKind::Tls)
            })?,
        };
        Ok(QuicConnection { connection })
    }

    pub async fn wait_idle(&self) {
        self.endpoint.wait_idle().await;
    }

    pub fn close(&self) {
        self.endpoint.close(0_u8.into(), b"endpoint stopped");
    }
}

fn endpoint_with_socket(
    socket: UdpSocket,
    server_config: Option<ServerConfig>,
) -> Result<Endpoint, QuicEndpointError> {
    socket
        .set_nonblocking(true)
        .map_err(|_| QuicEndpointError::Bind)?;
    Endpoint::new(
        EndpointConfig::default(),
        server_config,
        socket,
        Arc::new(TokioRuntime),
    )
    .map_err(|_| QuicEndpointError::Bind)
}

#[async_trait]
impl SecureConnector<SocketAddr> for QuicEndpoint {
    type Connection = QuicConnection;

    async fn connect(
        &self,
        endpoint: SocketAddr,
        cancellation: CancellationToken,
    ) -> Result<Self::Connection, ConnectAttemptError> {
        let connecting = self
            .endpoint
            .connect(endpoint, "halo.invalid")
            .map_err(|_| ConnectAttemptError::new(ConnectErrorKind::Unreachable))?;
        let connection = tokio::select! {
            () = cancellation.cancelled() => {
                return Err(ConnectAttemptError::new(ConnectErrorKind::Cancelled));
            }
            result = connecting => result.map_err(|_| {
                ConnectAttemptError::new(ConnectErrorKind::Tls)
            })?,
        };
        Ok(QuicConnection { connection })
    }
}

#[derive(Clone)]
pub struct QuicConnection {
    connection: Connection,
}

impl QuicConnection {
    #[must_use]
    pub fn remote_address(&self) -> SocketAddr {
        self.connection.remote_address()
    }
    pub async fn open_control(&self) -> Result<impl ControlIo + use<>, FrameIoError> {
        let (send, receive) = self
            .connection
            .open_bi()
            .await
            .map_err(|_| FrameIoError::Write)?;
        QuicControlIo::new(self.connection.clone(), send, receive)
    }

    pub async fn accept_control(&self) -> Result<impl ControlIo + use<>, FrameIoError> {
        let (send, receive) = self
            .connection
            .accept_bi()
            .await
            .map_err(|_| FrameIoError::Read)?;
        QuicControlIo::new(self.connection.clone(), send, receive)
    }

    pub async fn open_data(&self) -> Result<QuicDataIo, DataIoError> {
        let (send, receive) = self
            .connection
            .open_bi()
            .await
            .map_err(|_| DataIoError::Write)?;
        QuicDataIo::new(self.connection.clone(), send, receive)
    }

    pub async fn accept_data(&self) -> Result<QuicDataIo, DataIoError> {
        let (send, receive) = self
            .connection
            .accept_bi()
            .await
            .map_err(|_| DataIoError::Read)?;
        QuicDataIo::new(self.connection.clone(), send, receive)
    }

    pub fn close(&self) {
        self.connection.close(0_u8.into(), b"done");
    }

    /// Waits until QUIC reports that the connection can no longer carry new streams.
    pub async fn closed(&self) {
        let _ = self.connection.closed().await;
    }
}

struct QuicControlIo {
    send: SendStream,
    receive: RecvStream,
    binding: TlsChannelBinding,
}

impl QuicControlIo {
    fn new(
        connection: Connection,
        send: SendStream,
        receive: RecvStream,
    ) -> Result<Self, FrameIoError> {
        let mut exporter = [0_u8; 32];
        connection
            .export_keying_material(&mut exporter, EXPORTER_LABEL, b"")
            .map_err(|_| FrameIoError::Read)?;
        Ok(Self {
            send,
            receive,
            binding: TlsChannelBinding::new(exporter),
        })
    }
}

#[async_trait]
impl ControlIo for QuicControlIo {
    fn channel_binding(&self) -> TlsChannelBinding {
        self.binding
    }

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), FrameIoError> {
        if frame.len() > MAX_FRAME_LEN {
            return Err(FrameIoError::FrameTooLarge(frame.len()));
        }
        self.send
            .write_all(frame)
            .await
            .map_err(|_| FrameIoError::Write)
    }

    async fn receive_frame(&mut self, max_len: usize) -> Result<Vec<u8>, FrameIoError> {
        let limit = max_len.min(MAX_FRAME_LEN);
        let mut header = [0_u8; HEADER_LEN];
        self.receive
            .read_exact(&mut header)
            .await
            .map_err(|_| FrameIoError::Truncated)?;
        let payload_len =
            u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
        let frame_len = HEADER_LEN
            .checked_add(payload_len)
            .ok_or(FrameIoError::FrameTooLarge(usize::MAX))?;
        if frame_len > limit {
            return Err(FrameIoError::FrameTooLarge(frame_len));
        }
        let mut frame = vec![0_u8; frame_len];
        frame[..HEADER_LEN].copy_from_slice(&header);
        self.receive
            .read_exact(&mut frame[HEADER_LEN..])
            .await
            .map_err(|_| FrameIoError::Truncated)?;
        Ok(frame)
    }

    async fn close(&mut self) {
        let _ = self.send.finish();
        let _ = self.receive.stop(0_u8.into());
    }
}

pub struct QuicDataIo {
    send: SendStream,
    receive: RecvStream,
    binding: TlsChannelBinding,
}

impl QuicDataIo {
    fn new(
        connection: Connection,
        send: SendStream,
        receive: RecvStream,
    ) -> Result<Self, DataIoError> {
        let mut exporter = [0_u8; 32];
        connection
            .export_keying_material(&mut exporter, EXPORTER_LABEL, b"")
            .map_err(|_| DataIoError::Read)?;
        Ok(Self {
            send,
            receive,
            binding: TlsChannelBinding::new(exporter),
        })
    }
}

#[async_trait]
impl DataIo for QuicDataIo {
    fn channel_binding(&self) -> TlsChannelBinding {
        self.binding
    }

    async fn send_record(&mut self, record: &[u8]) -> Result<(), DataIoError> {
        if record.len() < DATA_RECORD_HEADER_LEN || record.len() > MAX_DATA_RECORD_LEN {
            return Err(DataIoError::RecordTooLarge(record.len()));
        }
        let declared = data_record_length(&record[..DATA_RECORD_HEADER_LEN])?;
        if declared != record.len() {
            return Err(DataIoError::RecordTooLarge(record.len()));
        }
        self.send
            .write_all(record)
            .await
            .map_err(|_| DataIoError::Write)
    }

    async fn receive_record(&mut self) -> Result<Vec<u8>, DataIoError> {
        let mut header = [0_u8; DATA_RECORD_HEADER_LEN];
        self.receive
            .read_exact(&mut header)
            .await
            .map_err(|_| DataIoError::Truncated)?;
        let record_length = data_record_length(&header)?;
        let mut record = vec![0_u8; record_length];
        record[..DATA_RECORD_HEADER_LEN].copy_from_slice(&header);
        self.receive
            .read_exact(&mut record[DATA_RECORD_HEADER_LEN..])
            .await
            .map_err(|_| DataIoError::Truncated)?;
        Ok(record)
    }

    async fn finish_send(&mut self) -> Result<(), DataIoError> {
        self.send.finish().map_err(|_| DataIoError::Write)
    }

    async fn expect_end(&mut self) -> Result<(), DataIoError> {
        let mut trailing = [0_u8; 1];
        match self
            .receive
            .read(&mut trailing)
            .await
            .map_err(|_| DataIoError::Read)?
        {
            None => Ok(()),
            Some(_) => Err(DataIoError::TrailingData),
        }
    }

    async fn close(&mut self) {
        let _ = self.send.finish();
        let _ = self.receive.stop(0_u8.into());
    }
}

fn client_config() -> Result<ClientConfig, QuicEndpointError> {
    let mut tls = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(PairingCertificateVerifier::new())
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.enable_early_data = false;
    let crypto = QuicClientConfig::try_from(tls).map_err(|_| QuicEndpointError::TlsConfig)?;
    let mut config = ClientConfig::new(Arc::new(crypto));
    config.transport_config(transport_config()?);
    Ok(config)
}

fn server_config() -> Result<ServerConfig, QuicEndpointError> {
    let certificate = rcgen::generate_simple_self_signed(vec!["halo.invalid".to_owned()])
        .map_err(|_| QuicEndpointError::Certificate)?;
    let certificate_der = CertificateDer::from(certificate.cert);
    let private_key = PrivatePkcs8KeyDer::from(certificate.signing_key.serialize_der());
    let mut tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certificate_der], private_key.into())
        .map_err(|_| QuicEndpointError::TlsConfig)?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.max_early_data_size = 0;
    let crypto = QuicServerConfig::try_from(tls).map_err(|_| QuicEndpointError::TlsConfig)?;
    let mut config = ServerConfig::with_crypto(Arc::new(crypto));
    config.migration(false);
    config.transport = transport_config()?;
    Ok(config)
}

fn transport_config() -> Result<Arc<TransportConfig>, QuicEndpointError> {
    let mut config = TransportConfig::default();
    config.max_concurrent_uni_streams(0_u8.into());
    config.max_concurrent_bidi_streams(4_u8.into());
    let idle_timeout = Duration::from_secs(75)
        .try_into()
        .map_err(|_| QuicEndpointError::TlsConfig)?;
    config.max_idle_timeout(Some(idle_timeout));
    Ok(Arc::new(config))
}

/// TLS validates possession of the ephemeral certificate key here. Halo device
/// authentication is completed by the exporter-bound signed pairing handshake.
#[derive(Debug)]
struct PairingCertificateVerifier(Arc<rustls::crypto::CryptoProvider>);

impl PairingCertificateVerifier {
    fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
    }
}

impl rustls::client::danger::ServerCertVerifier for PairingCertificateVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if end_entity.is_empty()
            || end_entity.len() > MAX_CERTIFICATE_LEN
            || !intermediates.is_empty()
        {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ));
        }
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum QuicEndpointError {
    #[error("failed to bind QUIC endpoint")]
    Bind,
    #[error("failed to read QUIC endpoint local address")]
    LocalAddress,
    #[error("failed to generate ephemeral TLS certificate")]
    Certificate,
    #[error("failed to configure QUIC/TLS")]
    TlsConfig,
}

impl fmt::Debug for QuicEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuicEndpoint")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use halo_protocol::{
        Capabilities, ClientHello, IDENTITY_KEY_LEN, NONCE_LEN, PairingMessage, ProtocolRange,
        SIGNATURE_LEN, TransferChunk,
    };

    use crate::{receive_message, send_message};

    use super::*;

    #[tokio::test]
    async fn loopback_quic_exports_same_binding_and_moves_bounded_frame() {
        let server_socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .unwrap_or_else(|error| panic!("server socket: {error}"));
        let server = Arc::new(
            QuicEndpoint::server_with_socket(server_socket)
                .unwrap_or_else(|error| panic!("server endpoint: {error}")),
        );
        let address = server
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
                let binding = io.channel_binding();
                let message = receive_message(&mut io)
                    .await
                    .unwrap_or_else(|error| panic!("receive message: {error}"));
                io.close().await;
                (binding, message)
            })
        };

        let client_socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .unwrap_or_else(|error| panic!("client socket: {error}"));
        let client = Arc::new(
            QuicEndpoint::client_with_socket(client_socket)
                .unwrap_or_else(|error| panic!("client endpoint: {error}")),
        );
        let connection = client
            .connect(address, CancellationToken::new())
            .await
            .unwrap_or_else(|error| panic!("connect: {error}"));
        let mut io = connection
            .open_control()
            .await
            .unwrap_or_else(|error| panic!("open control: {error}"));
        let client_binding = io.channel_binding();
        let message = PairingMessage::ClientHello(ClientHello {
            versions: ProtocolRange::new(1, 1)
                .unwrap_or_else(|error| panic!("test range: {error}")),
            capabilities: Capabilities::default(),
            nonce: [1; NONCE_LEN],
            identity_key: [2; IDENTITY_KEY_LEN],
            signature: [3; SIGNATURE_LEN],
        });
        send_message(&mut io, &message)
            .await
            .unwrap_or_else(|error| panic!("send message: {error}"));
        let (server_binding, received) = server_task
            .await
            .unwrap_or_else(|error| panic!("server task: {error}"));
        assert_eq!(client_binding, server_binding);
        assert_eq!(received, message);
    }

    #[tokio::test]
    async fn closing_control_stream_retains_connection_for_bounded_data_stream() {
        let server = Arc::new(
            QuicEndpoint::server(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .unwrap_or_else(|error| panic!("server endpoint: {error}")),
        );
        let address = server
            .local_addr()
            .unwrap_or_else(|error| panic!("server address: {error}"));
        let record = TransferChunk::new([1; 16], 0, [2; 32], vec![3; 1024])
            .and_then(|chunk| chunk.encode())
            .unwrap_or_else(|error| panic!("record: {error}"));
        let expected_record = record.clone();
        let (client_received, server_may_close) = tokio::sync::oneshot::channel();
        let server_task = {
            let server = Arc::clone(&server);
            tokio::spawn(async move {
                let connection = server
                    .accept(CancellationToken::new())
                    .await
                    .unwrap_or_else(|error| panic!("accept: {error}"));
                let mut control = connection
                    .accept_control()
                    .await
                    .unwrap_or_else(|error| panic!("accept control: {error}"));
                let control_binding = control.channel_binding();
                let _ = receive_message(&mut control)
                    .await
                    .unwrap_or_else(|error| panic!("receive control: {error}"));
                control.close().await;

                let mut data = connection
                    .accept_data()
                    .await
                    .unwrap_or_else(|error| panic!("accept data: {error}"));
                assert_eq!(data.channel_binding(), control_binding);
                let received = data
                    .receive_record()
                    .await
                    .unwrap_or_else(|error| panic!("receive data: {error}"));
                data.send_record(&received)
                    .await
                    .unwrap_or_else(|error| panic!("echo data: {error}"));
                data.close().await;
                let _ = server_may_close.await;
                connection.close();
                received
            })
        };

        let client = QuicEndpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .unwrap_or_else(|error| panic!("client endpoint: {error}"));
        let connection = client
            .connect(address, CancellationToken::new())
            .await
            .unwrap_or_else(|error| panic!("connect: {error}"));
        let mut control = connection
            .open_control()
            .await
            .unwrap_or_else(|error| panic!("open control: {error}"));
        let message = PairingMessage::ClientHello(ClientHello {
            versions: ProtocolRange::new(1, 1)
                .unwrap_or_else(|error| panic!("test range: {error}")),
            capabilities: Capabilities::default(),
            nonce: [1; NONCE_LEN],
            identity_key: [2; IDENTITY_KEY_LEN],
            signature: [3; SIGNATURE_LEN],
        });
        send_message(&mut control, &message)
            .await
            .unwrap_or_else(|error| panic!("send control: {error}"));
        let binding = control.channel_binding();
        control.close().await;

        let mut data = connection
            .open_data()
            .await
            .unwrap_or_else(|error| panic!("open data: {error}"));
        assert_eq!(data.channel_binding(), binding);
        data.send_record(&record)
            .await
            .unwrap_or_else(|error| panic!("send data: {error}"));
        let echoed = data
            .receive_record()
            .await
            .unwrap_or_else(|error| panic!("receive echo: {error}"));
        assert_eq!(echoed, record);
        let _ = client_received.send(());
        data.close().await;
        let received = server_task
            .await
            .unwrap_or_else(|error| panic!("server task: {error}"));
        assert_eq!(received, expected_record);
    }

    #[test]
    fn native_tls_identity_uses_apple_p256_external_representation() {
        let identity = generate_native_tls_identity()
            .unwrap_or_else(|error| panic!("native TLS identity: {error}"));
        assert!(!identity.certificate_der.is_empty());
        assert!(identity.certificate_der.len() <= MAX_CERTIFICATE_LEN);
        assert_eq!(identity.private_key_x963.len(), 97);
        assert_eq!(identity.private_key_x963[0], 4);
        let secret = SecretKey::from_slice(&identity.private_key_x963[65..])
            .unwrap_or_else(|error| panic!("native TLS secret: {error}"));
        assert_eq!(
            secret.public_key().to_sec1_bytes().as_ref(),
            &identity.private_key_x963[..65]
        );
    }
}
