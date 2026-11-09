//! TLS as adb has it: 1.3, the computer's key presented as a client certificate, and any
//! certificate taken from the phone.
//!
//! Taking any certificate is adb's own way. When pairing, what keeps a third party out is
//! SPAKE2's password including the TLS session's exported key, which a party in the middle
//! would not share; once paired, it is adbd that checks the computer, by its key.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};

use crate::key::AdbKey;

#[derive(Debug)]
struct AnyPhone(Vec<SignatureScheme>);

impl ServerCertVerifier for AnyPhone {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        // Any certificate is taken, so whether it signed the handshake proves nothing more.
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.clone()
    }
}

/// The client side of an adb TLS connection, presenting `key`.
pub(crate) fn client(key: &AdbKey) -> Result<Arc<ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let schemes = provider.signature_verification_algorithms.supported_schemes();
    let (cert, private) = key.certificate()?;
    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| format!("no TLS 1.3: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyPhone(schemes)))
        .with_client_auth_cert(vec![cert], private)
        .map_err(|e| format!("the key cannot be presented: {e}"))?;
    Ok(Arc::new(config))
}

/// A TLS connection over `socket`, its handshake done.
pub(crate) fn connect(
    key: &AdbKey,
    host: std::net::IpAddr,
    mut socket: std::net::TcpStream,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, std::net::TcpStream>, String> {
    let mut tls = rustls::ClientConnection::new(client(key)?, ServerName::IpAddress(host.into()))
        .map_err(|e| format!("TLS: {e}"))?;
    while tls.is_handshaking() {
        tls.complete_io(&mut socket).map_err(|e| format!("the TLS handshake: {e}"))?;
    }
    Ok(rustls::StreamOwned::new(tls, socket))
}
