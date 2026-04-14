//! SPKI TLS pinning — reject connections to servers with wrong certificate public key.
//!
//! Wraps standard WebPKI chain validation with an additional check that the
//! leaf certificate's SubjectPublicKeyInfo SHA-256 hash matches a compiled-in pin.

/// Primary SPKI pin — SHA-256 of the leaf cert's SubjectPublicKeyInfo DER.
/// Placeholder zeros: fill with real pin when production certificate is provisioned.
#[cfg(not(feature = "dev-store"))]
const PRIMARY_PIN: [u8; 32] = [0u8; 32];

/// Backup SPKI pin — for certificate rotation.
#[cfg(not(feature = "dev-store"))]
const BACKUP_PIN: [u8; 32] = [0u8; 32];

/// Build an HTTP client with SPKI-pinned TLS (production) or plain WebPKI (dev).
pub fn build_pinned_http_client() -> reqwest::Client {
    #[cfg(feature = "dev-store")]
    {
        // Dev mode: no TLS pinning (localhost uses plain HTTP)
        reqwest::Client::builder()
            .build()
            .expect("Failed to build HTTP client")
    }

    #[cfg(not(feature = "dev-store"))]
    {
        use std::sync::Arc;
        use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
        use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
        use rustls::{DigitallySignedStruct, Error, SignatureScheme};
        use sha2::{Sha256, Digest};

        /// Verifier that performs standard WebPKI chain validation + SPKI SHA-256 pin check.
        #[derive(Debug)]
        struct SpkiPinningVerifier {
            inner: Arc<rustls::client::WebPkiServerVerifier>,
        }

        impl SpkiPinningVerifier {
            fn new() -> Arc<Self> {
                let root_store = rustls::RootCertStore {
                    roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
                };
                let inner = rustls::client::WebPkiServerVerifier::builder(Arc::new(root_store))
                    .build()
                    .expect("Failed to build WebPKI verifier");
                Arc::new(Self { inner })
            }
        }

        impl ServerCertVerifier for SpkiPinningVerifier {
            fn verify_server_cert(
                &self,
                end_entity: &CertificateDer<'_>,
                intermediates: &[CertificateDer<'_>],
                server_name: &ServerName<'_>,
                ocsp_response: &[u8],
                now: UnixTime,
            ) -> Result<ServerCertVerified, Error> {
                // 1. Standard WebPKI chain validation
                self.inner.verify_server_cert(
                    end_entity, intermediates, server_name, ocsp_response, now,
                )?;

                // 2. SPKI pin check (skip if pins are placeholder zeros)
                if PRIMARY_PIN == [0u8; 32] && BACKUP_PIN == [0u8; 32] {
                    return Ok(ServerCertVerified::assertion());
                }

                let spki_hash = extract_spki_sha256(end_entity.as_ref());
                if spki_hash.as_ref() == Some(&PRIMARY_PIN) || spki_hash.as_ref() == Some(&BACKUP_PIN) {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(Error::General(format!(
                        "SPKI pin mismatch: got {:?}",
                        spki_hash.map(|h| hex::encode(&h))
                    )))
                }
            }

            fn verify_tls12_signature(
                &self,
                message: &[u8],
                cert: &CertificateDer<'_>,
                dss: &DigitallySignedStruct,
            ) -> Result<HandshakeSignatureValid, Error> {
                self.inner.verify_tls12_signature(message, cert, dss)
            }

            fn verify_tls13_signature(
                &self,
                message: &[u8],
                cert: &CertificateDer<'_>,
                dss: &DigitallySignedStruct,
            ) -> Result<HandshakeSignatureValid, Error> {
                self.inner.verify_tls13_signature(message, cert, dss)
            }

            fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
                self.inner.supported_verify_schemes()
            }
        }

        /// Extract SubjectPublicKeyInfo from a DER-encoded X.509 certificate,
        /// then return its SHA-256 hash.
        ///
        /// X.509 DER structure (simplified):
        ///   SEQUENCE (Certificate)
        ///     SEQUENCE (TBSCertificate)
        ///       [0] version (explicit tag, optional)
        ///       INTEGER (serialNumber)
        ///       SEQUENCE (signature algorithm)
        ///       SEQUENCE (issuer)
        ///       SEQUENCE (validity)
        ///       SEQUENCE (subject)
        ///       SEQUENCE (subjectPublicKeyInfo)  ← this is what we want
        fn extract_spki_sha256(cert_der: &[u8]) -> Option<[u8; 32]> {
            // Walk DER manually to find the 7th element of TBSCertificate
            let (tbs, _) = read_der_sequence(cert_der)?;
            let (tbs_inner, _) = read_der_sequence(tbs)?;

            let mut pos = 0;
            let inner = tbs_inner;

            // Field 0: version [0] EXPLICIT (optional — tag 0xA0)
            if inner.get(pos)? & 0xFF == 0xA0 {
                let (_, consumed) = read_der_element(&inner[pos..])?;
                pos += consumed;
            }

            // Field 1: serialNumber (INTEGER)
            let (_, consumed) = read_der_element(&inner[pos..])?;
            pos += consumed;

            // Field 2: signature (SEQUENCE)
            let (_, consumed) = read_der_element(&inner[pos..])?;
            pos += consumed;

            // Field 3: issuer (SEQUENCE)
            let (_, consumed) = read_der_element(&inner[pos..])?;
            pos += consumed;

            // Field 4: validity (SEQUENCE)
            let (_, consumed) = read_der_element(&inner[pos..])?;
            pos += consumed;

            // Field 5: subject (SEQUENCE)
            let (_, consumed) = read_der_element(&inner[pos..])?;
            pos += consumed;

            // Field 6: subjectPublicKeyInfo (SEQUENCE) — raw DER including tag+length
            let (_, spki_len) = read_der_element(&inner[pos..])?;
            let spki_der = &inner[pos..pos + spki_len];

            let hash: [u8; 32] = Sha256::digest(spki_der).into();
            Some(hash)
        }

        /// Read a DER SEQUENCE, return (inner content bytes, total consumed bytes).
        fn read_der_sequence(data: &[u8]) -> Option<(&[u8], usize)> {
            if data.first()? & 0x1F != 0x10 {
                return None; // Not a SEQUENCE tag (0x30)
            }
            let (content, total) = read_der_element(data)?;
            Some((content, total))
        }

        /// Read any DER element: return (content slice, total bytes consumed including tag+length).
        fn read_der_element(data: &[u8]) -> Option<(&[u8], usize)> {
            if data.len() < 2 {
                return None;
            }
            let mut pos = 1; // skip tag byte
            let length_byte = data[pos];
            pos += 1;

            let content_len = if length_byte & 0x80 == 0 {
                length_byte as usize
            } else {
                let num_octets = (length_byte & 0x7F) as usize;
                if num_octets > 4 || pos + num_octets > data.len() {
                    return None;
                }
                let mut len = 0usize;
                for i in 0..num_octets {
                    len = (len << 8) | (data[pos + i] as usize);
                }
                pos += num_octets;
                len
            };

            if pos + content_len > data.len() {
                return None;
            }

            let total = pos + content_len;
            Some((&data[pos..total], total))
        }

        // Hex encoding helper (avoid pulling in another crate)
        mod hex {
            pub fn encode(bytes: &[u8; 32]) -> String {
                bytes.iter().map(|b| format!("{:02x}", b)).collect()
            }
        }

        let verifier = SpkiPinningVerifier::new();
        let tls_config = rustls::ClientConfig::builder_with_provider(
                Arc::new(rustls::crypto::ring::default_provider()),
            )
            .with_safe_default_protocol_versions()
            .expect("Failed to set TLS protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();

        reqwest::Client::builder()
            .use_preconfigured_tls(tls_config)
            .build()
            .expect("Failed to build pinned HTTP client")
    }
}
