use std::{fmt, net::SocketAddr, sync::Arc, time::Duration};

use async_trait::async_trait;
use quinn::{
    ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig, TransportConfig,
    crypto::rustls::{QuicClientConfig, QuicServerConfig},
};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use halo_crypto::TlsChannelBinding;
use halo_protocol::{HEADER_LEN, MAX_FRAME_LEN};

use crate::{ConnectAttemptError, ConnectErrorKind, ControlIo, FrameIoError, SecureConnector};

const ALPN: &[u8] = b"halo-pairing/1";
const EXPORTER_LABEL: &[u8] = b"EXPORTER-Halo-Pairing-v1";
const MAX_CERTIFICATE_LEN: usize = 16 * 1024;

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

    pub fn close(&self) {
        self.connection.close(0_u8.into(), b"done");
    }
}

struct QuicControlIo {
    connection: Connection,
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
            connection,
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
        self.connection.close(0_u8.into(), b"control closed");
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
        SIGNATURE_LEN,
    };

    use crate::{receive_message, send_message};

    use super::*;

    #[tokio::test]
    async fn loopback_quic_exports_same_binding_and_moves_bounded_frame() {
        let server = Arc::new(
            QuicEndpoint::server(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
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

        let client = Arc::new(
            QuicEndpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
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
}
