//! AwsSigV4Delegator — Engine C: AWS Signature Version 4 request signing.
//!
//! Computes a cryptographic signature over the canonical request (method, path,
//! query, headers, body hash) using the stored AWS key pair, then injects
//! `Authorization`, `x-amz-date`, `x-amz-content-sha256`, and optionally
//! `x-amz-security-token` headers.
//!
//! Credentials are resolved from `domain_token_accounts` using named tokens:
//! - `AWS_ACCESS_KEY_ID` → AWS access key ID
//! - `AWS_SECRET_ACCESS_KEY` → AWS secret access key
//! - `AWS_REGION` → AWS region
//! - `session_token` → (optional) temporary session token

use super::AuthDelegator;
use crate::config_store::ConfigStore;
use crate::keystore::KeyStore;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::sync::Arc;

type HmacSha256 = Hmac<Sha256>;

pub struct AwsSigV4Delegator {
    keystore: Arc<dyn KeyStore>,
    config_store: Arc<ConfigStore>,
    /// Explicit token names to use instead of looking up from domain_token_accounts.
    /// Used for allow_once decisions where tokens are not persisted in DB.
    token_names_override: Option<Vec<String>>,
}

impl AwsSigV4Delegator {
    pub fn new(keystore: Arc<dyn KeyStore>, config_store: Arc<ConfigStore>) -> Self {
        Self { keystore, config_store, token_names_override: None }
    }

    /// Create a delegator that uses the given token names instead of querying DB.
    /// Used by allow_once paths where token names are held in-memory.
    pub fn with_token_names(
        keystore: Arc<dyn KeyStore>,
        config_store: Arc<ConfigStore>,
        names: Vec<String>,
    ) -> Self {
        Self { keystore, config_store, token_names_override: Some(names) }
    }
}

#[async_trait]
impl AuthDelegator for AwsSigV4Delegator {
    fn kind(&self) -> &str {
        "aws-sigv4"
    }

    async fn apply_auth(
        &self,
        request: &mut reqwest::Request,
        domain: &str,
        _credential_account: &str,
    ) -> Result<()> {
        // 1. Load credentials: use override names (allow_once) or query DB (allow_always)
        let token_names = if let Some(ref names) = self.token_names_override {
            names.clone()
        } else {
            self.config_store
                .list_domain_tokens(domain)
                .map_err(|e| anyhow!("Failed to list domain tokens for {}: {}", domain, e))?
        };

        let mut access_key_id: Option<String> = None;
        let mut secret_access_key: Option<String> = None;
        let mut region: Option<String> = None;
        let mut session_token: Option<String> = None;

        for token_name in &token_names {
            match self.keystore.get(token_name).await {
                Ok(value) => match token_name.as_str() {
                    "AWS_ACCESS_KEY_ID" => access_key_id = Some(value),
                    "AWS_SECRET_ACCESS_KEY" => secret_access_key = Some(value),
                    "AWS_REGION" => region = Some(value),
                    "session_token" => session_token = Some(value),
                    _ => {}
                },
                Err(e) => {
                    tracing::warn!("Failed to load token '{}' for {}: {}", token_name, domain, e);
                }
            }
        }

        let access_key_id = access_key_id
            .ok_or_else(|| anyhow!("Missing 'AWS_ACCESS_KEY_ID' token for domain {}", domain))?;
        let secret_access_key = secret_access_key
            .ok_or_else(|| anyhow!("Missing 'AWS_SECRET_ACCESS_KEY' token for domain {}", domain))?;

        // 2. Extract service name and region from domain
        let host = request
            .url()
            .host_str()
            .ok_or_else(|| anyhow!("Request URL has no host"))?
            .to_string();
        let service = extract_aws_service(&host);

        // Region priority: keystore token → hostname → error
        let region = region
            .or_else(|| extract_region_from_host(&host))
            .ok_or_else(|| anyhow!("Cannot determine AWS region for domain {} (set AWS_REGION token or use regional endpoint)", domain))?;

        // 3. Remove any existing authorization header (we compute our own)
        request.headers_mut().remove("authorization");

        // 4. Timestamps
        let now = chrono::Utc::now();
        let date_stamp = now.format("%Y%m%d").to_string();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();

        // 5. Body hash
        let body_bytes = request.body().and_then(|b| b.as_bytes()).unwrap_or(b"");
        let body_hash = hex_sha256(body_bytes);

        // 6. Add required SigV4 headers
        request.headers_mut().insert("host", host.parse()?);
        request
            .headers_mut()
            .insert("x-amz-date", amz_date.parse()?);
        request
            .headers_mut()
            .insert("x-amz-content-sha256", body_hash.parse()?);
        if let Some(ref token) = session_token {
            request
                .headers_mut()
                .insert("x-amz-security-token", token.parse()?);
        }

        // 7. Determine signed headers (sorted, lowercase)
        let mut signed_header_names: Vec<String> = vec![
            "host".to_string(),
            "x-amz-content-sha256".to_string(),
            "x-amz-date".to_string(),
        ];
        if session_token.is_some() {
            signed_header_names.push("x-amz-security-token".to_string());
        }
        if request.headers().contains_key("content-type") {
            signed_header_names.push("content-type".to_string());
        }
        signed_header_names.sort();
        let signed_headers_str = signed_header_names.join(";");

        // 8. Build canonical headers
        let mut canonical_headers = String::new();
        for name in &signed_header_names {
            let value = request
                .headers()
                .get(name.as_str())
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .trim();
            canonical_headers.push_str(&format!("{}:{}\n", name, value));
        }

        // 9. Build canonical request
        let method = request.method().as_str();
        let c_uri = canonical_uri(request.url().path());
        let c_query = canonical_query_string(request.url());

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method, c_uri, c_query, canonical_headers, signed_headers_str, body_hash,
        );

        // 10. Build string to sign
        let scope = format!("{}/{}/{}/aws4_request", date_stamp, region, service);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date,
            scope,
            hex_sha256(canonical_request.as_bytes()),
        );

        // 11. Derive signing key
        let signing_key =
            derive_signing_key(&secret_access_key, &date_stamp, &region, &service);

        // 12. Compute signature
        let signature = hex_hmac_sha256(&signing_key, string_to_sign.as_bytes());

        // 13. Build and inject Authorization header
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            access_key_id, scope, signed_headers_str, signature,
        );
        request
            .headers_mut()
            .insert("authorization", authorization.parse()?);

        Ok(())
    }

    fn credential_description(&self) -> &str {
        "AWS Credentials (AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY + AWS_REGION)"
    }
}

// ---------------------------------------------------------------------------
// SigV4 helper functions
// ---------------------------------------------------------------------------

/// Extract AWS region from a regional endpoint hostname.
///
/// e.g., "bedrock-runtime.us-east-1.amazonaws.com" → Some("us-east-1")
/// e.g., "s3.ap-northeast-2.amazonaws.com"        → Some("ap-northeast-2")
/// e.g., "iam.amazonaws.com"                       → None  (global service)
fn extract_region_from_host(host: &str) -> Option<String> {
    let stripped = host
        .strip_suffix(".amazonaws.com")
        .or_else(|| host.strip_suffix(".amazonaws.com.cn"))?;
    // Format: <service>.<region>  — region is the second segment
    let mut parts = stripped.splitn(2, '.');
    parts.next(); // service prefix
    let region = parts.next()?;
    if region.is_empty() {
        None
    } else {
        Some(region.to_string())
    }
}

/// Extract AWS service name from hostname.
///
/// e.g., "bedrock-runtime.us-east-1.amazonaws.com" → "bedrock"
/// e.g., "s3.us-east-1.amazonaws.com" → "s3"
/// e.g., "iam.amazonaws.com" → "iam"
///
/// Some AWS services have a hostname prefix that differs from the SigV4 signing
/// service name. The normalization table below maps those cases.
fn extract_aws_service(host: &str) -> String {
    let stripped = host
        .strip_suffix(".amazonaws.com")
        .or_else(|| host.strip_suffix(".amazonaws.com.cn"))
        .unwrap_or(host);

    // First segment before any dot is the endpoint prefix
    let prefix = stripped
        .split('.')
        .next()
        .unwrap_or("execute-api");

    // Normalize endpoint prefix → SigV4 signing service name where they differ
    match prefix {
        "bedrock-runtime"       => "bedrock".to_string(),
        "bedrock-agent-runtime" => "bedrock".to_string(),
        other                   => other.to_string(),
    }
}

/// Compute SHA-256 hash and return as lowercase hex string.
fn hex_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex_encode(&hasher.finalize())
}

/// Compute HMAC-SHA256 and return raw bytes.
fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Compute HMAC-SHA256 and return as lowercase hex string.
fn hex_hmac_sha256(key: &[u8], data: &[u8]) -> String {
    hex_encode(&hmac_sha256(key, data))
}

/// Derive the SigV4 signing key via 4 rounds of HMAC-SHA256.
///
/// ```text
/// kDate    = HMAC("AWS4" + secret, dateStamp)
/// kRegion  = HMAC(kDate, region)
/// kService = HMAC(kRegion, service)
/// kSigning = HMAC(kService, "aws4_request")
/// ```
fn derive_signing_key(secret: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{}", secret).as_bytes(), date_stamp.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// Encode bytes as lowercase hex string.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// URI-encode a path for SigV4 canonical URI.
/// Each path segment is percent-encoded; '/' separators are preserved.
fn canonical_uri(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }
    path.split('/')
        .map(|segment| uri_encode(segment, false))
        .collect::<Vec<_>>()
        .join("/")
}

/// Build canonical query string: parameters sorted by name, then by value.
fn canonical_query_string(url: &reqwest::Url) -> String {
    let query = url.query().unwrap_or("");
    if query.is_empty() {
        return String::new();
    }
    let mut params: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (uri_encode(&k, true), uri_encode(&v, true)))
        .collect();
    params.sort();
    params
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&")
}

/// AWS SigV4 URI percent-encoding.
///
/// Encodes all characters except unreserved: `A-Z a-z 0-9 - . _ ~`.
/// If `encode_slash` is true, '/' is also encoded (used for query parameters).
fn uri_encode(input: &str, encode_slash: bool) -> String {
    let mut result = String::with_capacity(input.len() * 2);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                result.push(byte as char);
            }
            b'/' if !encode_slash => {
                result.push('/');
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Primitive helpers
    // -----------------------------------------------------------------------

    #[test]
    fn test_hex_sha256_empty() {
        // SHA-256("") — canonical body hash for GET / requests with no body
        assert_eq!(
            hex_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_hmac_sha256_rfc4231_tc2() {
        // RFC 4231 test case 2 — "what do ya want for nothing?"
        assert_eq!(
            hex_hmac_sha256(b"Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    // -----------------------------------------------------------------------
    // URI / query-string encoding
    // -----------------------------------------------------------------------

    #[test]
    fn test_uri_encode() {
        assert_eq!(uri_encode("hello world", true), "hello%20world");
        assert_eq!(uri_encode("foo/bar", true), "foo%2Fbar");
        assert_eq!(uri_encode("foo/bar", false), "foo/bar");
        assert_eq!(uri_encode("a-b_c.d~e", true), "a-b_c.d~e");
        // Tilde must NOT be encoded (unreserved per RFC 3986)
        assert_eq!(uri_encode("~", true), "~");
        // Chars that MUST be encoded
        assert_eq!(uri_encode("a=b", true), "a%3Db");
        assert_eq!(uri_encode("a+b", true), "a%2Bb");
    }

    #[test]
    fn test_canonical_uri() {
        assert_eq!(canonical_uri(""), "/");
        assert_eq!(canonical_uri("/"), "/");
        assert_eq!(canonical_uri("/foo/bar"), "/foo/bar");
        assert_eq!(canonical_uri("/foo bar/baz"), "/foo%20bar/baz");
    }

    #[test]
    fn test_canonical_query_string_sorting() {
        // AWS test suite — get-vanilla-query-order-keys
        // Params must be sorted lexicographically by name.
        // Ref: https://docs.aws.amazon.com/general/latest/gr/sigv4-create-canonical-request.html
        let url = reqwest::Url::parse(
            "https://example.amazonaws.com/?Param2=value2&Param1=value1",
        )
        .unwrap();
        assert_eq!(canonical_query_string(&url), "Param1=value1&Param2=value2");
    }

    #[test]
    fn test_canonical_query_string_same_key_sorted_by_value() {
        // AWS spec: if two params share the same name, sort by value.
        let url = reqwest::Url::parse(
            "https://example.amazonaws.com/?a=z&a=a",
        )
        .unwrap();
        assert_eq!(canonical_query_string(&url), "a=a&a=z");
    }

    #[test]
    fn test_canonical_query_string_special_chars() {
        // Space must be encoded as %20, not '+', in SigV4 query strings.
        let url = reqwest::Url::parse(
            "https://example.amazonaws.com/?msg=hello%20world",
        )
        .unwrap();
        assert_eq!(canonical_query_string(&url), "msg=hello%20world");
    }

    #[test]
    fn test_canonical_query_string_empty() {
        let url = reqwest::Url::parse("https://example.amazonaws.com/").unwrap();
        assert_eq!(canonical_query_string(&url), "");
    }

    // -----------------------------------------------------------------------
    // Service extraction
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_aws_service() {
        // bedrock-runtime endpoint → signing service name is "bedrock"
        assert_eq!(
            extract_aws_service("bedrock-runtime.us-east-1.amazonaws.com"),
            "bedrock"
        );
        assert_eq!(
            extract_aws_service("bedrock-agent-runtime.us-east-1.amazonaws.com"),
            "bedrock"
        );
        assert_eq!(extract_aws_service("s3.us-east-1.amazonaws.com"), "s3");
        assert_eq!(
            extract_aws_service("lambda.us-east-1.amazonaws.com"),
            "lambda"
        );
        assert_eq!(extract_aws_service("iam.amazonaws.com"), "iam");
        assert_eq!(extract_aws_service("sts.amazonaws.com"), "sts");
        assert_eq!(
            extract_aws_service("sts.cn-north-1.amazonaws.com.cn"),
            "sts"
        );
    }

    // -----------------------------------------------------------------------
    // AWS official test vectors
    // Ref: https://docs.aws.amazon.com/general/latest/gr/sigv4-calculate-signature.html
    // Credentials: AKIDEXAMPLE / wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY
    // -----------------------------------------------------------------------

    /// Signing key derivation — AWS SigV4 example credentials, service = "iam".
    ///
    /// Derivation chain:
    ///   kDate    = HMAC("AWS4wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "20150830")
    ///   kRegion  = HMAC(kDate, "us-east-1")
    ///   kService = HMAC(kRegion, "iam")
    ///   kSigning = HMAC(kService, "aws4_request")
    ///
    /// Expected value verified against our RFC 4231-compliant HMAC implementation.
    #[test]
    fn test_derive_signing_key_aws_vector() {
        let key = derive_signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830",
            "us-east-1",
            "iam",
        );
        assert_eq!(
            hex_encode(&key),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
    }

    /// Canonical request hash — AWS test suite case "get-vanilla".
    ///
    /// Request:
    ///   GET /?Param1=value1&Param2=value2 HTTP/1.1
    ///   Host: example.amazonaws.com
    ///   X-Amz-Date: 20150830T123600Z
    ///
    /// The canonical request string (explicit newlines shown as \n):
    ///   GET\n/\nParam1=value1&Param2=value2\n
    ///   host:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\n
    ///   host;x-amz-date\n
    ///   e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
    ///
    /// SHA-256 of the above canonical request must equal the value below.
    #[test]
    fn test_canonical_request_hash_get_vanilla() {
        let canonical_request = concat!(
            "GET\n",
            "/\n",
            "Param1=value1&Param2=value2\n",
            "host:example.amazonaws.com\n",
            "x-amz-date:20150830T123600Z\n",
            "\n",                        // blank line after last header
            "host;x-amz-date\n",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
        assert_eq!(
            hex_sha256(canonical_request.as_bytes()),
            "816cd5b414d056048ba4f7c5386d6e0533120fb1fcfa93762cf0fc39e2cf19e0"
        );
    }

    /// Full end-to-end signature — AWS test suite case "get-vanilla".
    ///
    /// Given the canonical request hash from `test_canonical_request_hash_get_vanilla`,
    /// the string to sign is:
    ///   AWS4-HMAC-SHA256\n
    ///   20150830T123600Z\n
    ///   20150830/us-east-1/service/aws4_request\n
    ///   816cd5b414d056048ba4f7c5386d6e0533120fb1fcfa93762cf0fc39e2cf19e0
    ///
    /// Signing key uses service = "service" (as in the test suite, not "iam").
    /// Expected final signature is taken from the AWS test suite output files.
    #[test]
    fn test_full_signature_get_vanilla() {
        let signing_key = derive_signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830",
            "us-east-1",
            "service",
        );
        let string_to_sign = concat!(
            "AWS4-HMAC-SHA256\n",
            "20150830T123600Z\n",
            "20150830/us-east-1/service/aws4_request\n",
            "816cd5b414d056048ba4f7c5386d6e0533120fb1fcfa93762cf0fc39e2cf19e0",
        );
        assert_eq!(
            hex_hmac_sha256(&signing_key, string_to_sign.as_bytes()),
            "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
        );
    }
}
