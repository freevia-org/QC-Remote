//! Proving a loopback endpoint is the QC Remote that published it, before the
//! bearer token is sent there.
//!
//! The local MCP descriptor names a `127.0.0.1` port and the token for it. When
//! the app exits without cleaning up (a crash, a forced stop - Drop never runs
//! under Tauri), the descriptor stays on disk for up to its staleness window
//! while the port is free for any local process to bind. A bridge that dialled
//! it then handed the squatter its bearer token and every MCP request, and took
//! back whatever answer the squatter chose to forge.
//!
//! So a descriptor now also names an identity endpoint. Before a request goes
//! out, the client sends a fresh random nonce there, unauthenticated, and the
//! server answers with its instance id and an HMAC of both under the bearer
//! token. Only the process that holds the token can produce it, and the token
//! itself never leaves the client until the proof checks out. A descriptor
//! without an identity endpoint comes from a release that predates this and is
//! dialled as before.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;
use subtle::ConstantTimeEq as _;

/// The path every identity endpoint is served at.
pub(crate) const IDENTITY_PATH: &str = "/identity";
/// How long a client waits for a proof.
const IDENTITY_TIMEOUT: Duration = Duration::from_secs(2);
// Software envelope budget, not a firmware limit. A proof is two short
// strings; untrusted listeners must be rejected before buffering arbitrary JSON.
const MAX_PROOF_BYTES: usize = 4096;
const PROOF_CONTEXT: &[u8] = b"qc-remote-local-identity-v1";
/// Nonces are 32 random bytes; anything much longer is not a nonce.
const MAX_NONCE_CHARACTERS: usize = 128;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdentityChallenge {
    pub(crate) nonce: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdentityProof {
    pub(crate) instance_id: String,
    pub(crate) proof: String,
}

/// HMAC-SHA256 (RFC 2104) over `parts`, joined with NUL separators so no two
/// different (instance, nonce) pairs share an input.
fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = [0_u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|byte| byte ^ 0x36));
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            inner.update([0_u8]);
        }
        inner.update(part);
    }
    let mut outer = Sha256::new();
    outer.update(block.map(|byte| byte ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// The proof a server holding `token` gives for `nonce`.
pub(crate) fn prove(token: &str, instance_id: &str, nonce: &str) -> Option<String> {
    if nonce.is_empty() || nonce.len() > MAX_NONCE_CHARACTERS {
        return None;
    }
    Some(URL_SAFE_NO_PAD.encode(hmac_sha256(
        token.as_bytes(),
        &[PROOF_CONTEXT, instance_id.as_bytes(), nonce.as_bytes()],
    )))
}

fn fresh_nonce() -> String {
    use rand::Rng as _;
    let mut bytes = [0_u8; 32];
    rand::rng().fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Whether `answer` proves possession of `token` for `instance_id` and `nonce`.
fn accepts(token: &str, instance_id: &str, nonce: &str, answer: &IdentityProof) -> bool {
    let Some(expected) = prove(token, instance_id, nonce) else {
        return false;
    };
    !instance_id.is_empty()
        && answer.instance_id == instance_id
        && bool::from(expected.as_bytes().ct_eq(answer.proof.as_bytes()))
}

/// Why an endpoint was not trusted with the token.
#[derive(Debug)]
pub(crate) struct IdentityFailure {
    /// The listener answered as the published instance, but its proof was
    /// made with another token: the descriptor and the listener disagree
    /// about the credential, or a squatter is echoing the instance id.
    pub(crate) credential_mismatch: bool,
    reason: String,
}

impl std::fmt::Display for IdentityFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl From<IdentityFailure> for String {
    fn from(failure: IdentityFailure) -> Self {
        failure.reason
    }
}

fn no_proof(reason: String) -> IdentityFailure {
    IdentityFailure {
        credential_mismatch: false,
        reason,
    }
}

/// Ask `identity_url` to prove it holds `token` as `instance_id`.
///
/// Sends nothing secret: only a fresh nonce. Any failure - no answer, a
/// malformed one, the wrong instance, a wrong proof - is an error, and the
/// caller must not send the token.
pub(crate) async fn verify(
    client: &reqwest::Client,
    identity_url: &str,
    instance_id: &str,
    token: &str,
) -> Result<(), IdentityFailure> {
    let nonce = fresh_nonce();
    let mut response = client
        .post(identity_url)
        .timeout(IDENTITY_TIMEOUT)
        .json(&IdentityChallenge {
            nonce: nonce.clone(),
        })
        .send()
        .await
        .map_err(|error| {
            no_proof(format!(
                "the published endpoint did not prove its identity ({error})"
            ))
        })?
        .error_for_status()
        .map_err(|error| {
            no_proof(format!(
                "the published endpoint refused the identity check ({error})"
            ))
        })?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PROOF_BYTES as u64)
    {
        return Err(no_proof(
            "the published identity proof exceeds its software envelope budget".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(MAX_PROOF_BYTES);
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        no_proof(format!(
            "the published endpoint gave no identity proof ({error})"
        ))
    })? {
        if chunk.len() > MAX_PROOF_BYTES - bytes.len() {
            return Err(no_proof(
                "the published identity proof exceeds its software envelope budget".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let answer: IdentityProof = serde_json::from_slice(&bytes).map_err(|error| {
        no_proof(format!(
            "the published endpoint gave no identity proof ({error})"
        ))
    })?;
    if accepts(token, instance_id, &nonce, &answer) {
        Ok(())
    } else if !instance_id.is_empty() && answer.instance_id == instance_id {
        Err(IdentityFailure {
            credential_mismatch: true,
            reason: "the listener on the published port could not prove it holds the published credential".into(),
        })
    } else {
        Err(no_proof(
            "the process on the published port is not the QC Remote that published it".into(),
        ))
    }
}

/// The identity endpoint that belongs to `rpc_url`: same scheme, host and
/// port, at [`IDENTITY_PATH`]. A descriptor naming any other place is refused,
/// so a tampered copy cannot send the check somewhere the real server is not.
pub(crate) fn identity_url_matches(rpc_url: &reqwest::Url, identity_url: &str) -> bool {
    reqwest::Url::parse(identity_url).is_ok_and(|identity| {
        identity.scheme() == rpc_url.scheme()
            && identity.host_str() == rpc_url.host_str()
            && identity.port() == rpc_url.port()
            && identity.path() == IDENTITY_PATH
            && identity.query().is_none()
            && identity.fragment().is_none()
            && identity.username().is_empty()
            && identity.password().is_none()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn untrusted_identity_envelopes_are_bounded_before_json_or_credential_use() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for scenario in ["declared", "chunked", "refused"] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/identity", listener.local_addr().unwrap());
            let client = crate::loopback_http::loopback_client_builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            let server = async {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 8192];
                assert!(socket.read(&mut request).await.unwrap() > 0);
                let response = match scenario {
                    "declared" => "HTTP/1.1 200 OK\r\nContent-Length: 4097\r\n\r\n".to_owned(),
                    "chunked" => format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1001\r\n{}\r\n0\r\n\r\n", "x".repeat(4097)),
                    _ => "HTTP/1.1 403 Forbidden\r\nContent-Length: 999999999\r\n\r\n".to_owned(),
                };
                let _ = socket.write_all(response.as_bytes()).await;
            };
            // Joined futures, no detached server thread/task. The whole fixture
            // is bounded even if verification accidentally tries to read a body.
            let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
                tokio::join!(
                    verify(&client, &url, "fixture-instance", "fixture-token"),
                    server
                )
            })
            .await
            .unwrap();
            let failure = result.unwrap_err();
            assert!(!failure.credential_mismatch);
            if scenario == "refused" {
                assert!(failure.to_string().contains("refused the identity check"));
            } else {
                assert!(
                    failure.to_string().contains("software envelope budget"),
                    "{scenario}: {failure}"
                );
            }
        }
    }

    #[test]
    fn hmac_matches_the_rfc_4231_vectors() {
        fn hex(bytes: &[u8]) -> String {
            bytes.iter().map(|byte| format!("{byte:02x}")).collect()
        }
        // Test case 2: key "Jefe".
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", &[b"what do ya want for nothing?"])),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Test case 6: a key longer than the block is hashed first.
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                &[b"Test Using Larger Than Block-Size Key - Hash Key First"]
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn only_the_token_holder_answering_as_the_published_instance_is_accepted() {
        let token = "T".repeat(43);
        let nonce = fresh_nonce();
        let genuine = IdentityProof {
            instance_id: "instance-a".into(),
            proof: prove(&token, "instance-a", &nonce).unwrap(),
        };
        assert!(accepts(&token, "instance-a", &nonce, &genuine));
        // A different nonce, instance or token never verifies.
        assert!(!accepts(&token, "instance-a", &fresh_nonce(), &genuine));
        assert!(!accepts(&token, "instance-b", &nonce, &genuine));
        assert!(!accepts(&"U".repeat(43), "instance-a", &nonce, &genuine));
        // Echoing the published instance id without the token proves nothing.
        let squatter = IdentityProof {
            instance_id: "instance-a".into(),
            proof: prove(&"U".repeat(43), "instance-a", &nonce).unwrap(),
        };
        assert!(!accepts(&token, "instance-a", &nonce, &squatter));
        assert!(!accepts(&token, "", &nonce, &genuine));
        assert_ne!(fresh_nonce(), fresh_nonce());
        assert!(prove(&token, "instance-a", &"n".repeat(129)).is_none());
        assert!(prove(&token, "instance-a", "").is_none());
    }

    #[test]
    fn the_identity_endpoint_must_share_the_rpc_origin() {
        let rpc = reqwest::Url::parse("http://127.0.0.1:41006/rpc").unwrap();
        assert!(identity_url_matches(
            &rpc,
            "http://127.0.0.1:41006/identity"
        ));
        for other in [
            "http://127.0.0.1:41007/identity",
            "http://localhost:41006/identity",
            "https://127.0.0.1:41006/identity",
            "http://127.0.0.1:41006/rpc",
            "http://127.0.0.1:41006/identity?x=1",
            "http://user@127.0.0.1:41006/identity",
            "not a url",
        ] {
            assert!(!identity_url_matches(&rpc, other), "{other}");
        }
    }
}
