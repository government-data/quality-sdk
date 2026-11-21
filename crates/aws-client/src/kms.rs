//! The index's signing key in KMS (decision 0001): `Sign` of a SHA-256
//! digest (`MessageType=DIGEST`, since KMS signs a `RAW` message only up to
//! 4 KiB) and `GetPublicKey`. Only the publish role may call them.

use std::rc::Rc;

use base64::Engine;
use serde::Deserialize;
use serde_json::json;

use crate::client::{Account, AwsError};
use crate::http::Request;

/// A KMS signing key: ECC P-256, `ECDSA_SHA_256`.
#[derive(Debug, Clone)]
pub struct Kms {
    account: Rc<Account>,
    key_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Signed {
    signature: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PublicKey {
    public_key: String,
}

impl Kms {
    /// `key_id`: the key's id, ARN, alias (`alias/<name>`) or alias ARN.
    pub fn new(account: Rc<Account>, key_id: &str) -> Self {
        Self {
            account,
            key_id: key_id.into(),
        }
    }

    /// The DER ECDSA signature of `digest`, the SHA-256 of the message.
    pub fn sign_digest(&self, digest: &[u8; 32]) -> Result<Vec<u8>, AwsError> {
        let body = json!({
            "KeyId": self.key_id,
            "Message": b64().encode(digest),
            "MessageType": "DIGEST",
            "SigningAlgorithm": "ECDSA_SHA_256",
        });
        let what = format!("KMS Sign with {}", self.key_id);
        let answer: Signed = self.call("Sign", &body, &what)?;
        b64()
            .decode(answer.signature)
            .map_err(|_| AwsError::Failed(format!("{what}: the signature is not base64")))
    }

    /// The key's public key, DER SubjectPublicKeyInfo.
    pub fn public_key(&self) -> Result<Vec<u8>, AwsError> {
        let what = format!("KMS GetPublicKey of {}", self.key_id);
        let body = json!({ "KeyId": self.key_id });
        let answer: PublicKey = self.call("GetPublicKey", &body, &what)?;
        b64()
            .decode(answer.public_key)
            .map_err(|_| AwsError::Failed(format!("{what}: the key is not base64")))
    }

    fn call<T: for<'de> Deserialize<'de>>(
        &self,
        action: &str,
        body: &serde_json::Value,
        what: &str,
    ) -> Result<T, AwsError> {
        let url = format!("https://kms.{}.amazonaws.com/", self.account.region());
        let mut request = Request::new("POST", &url);
        request.headers.extend([
            ("content-type".into(), "application/x-amz-json-1.1".into()),
            ("x-amz-target".into(), format!("TrentService.{action}")),
        ]);
        request.body = body.to_string().into_bytes();
        let r = self.account.send("kms", request, what)?;
        serde_json::from_slice(&r.body)
            .map_err(|_| AwsError::Failed(format!("{what}: not its answer")))
    }
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{account, FakeAws};

    #[test]
    fn a_digest_is_signed_as_a_digest_and_the_der_returned() {
        let aws = FakeAws::default();
        aws.answer(
            "POST https://kms.us-east-1.amazonaws.com/",
            200,
            br#"{"KeyId":"arn:k","Signature":"MEUCIQ==","SigningAlgorithm":"ECDSA_SHA_256"}"#,
        );
        let kms = Kms::new(account(&aws), "alias/signing");
        assert_eq!(kms.sign_digest(&[7; 32]).unwrap(), [0x30, 0x45, 0x02, 0x21]);
        let sent = aws.sent.borrow();
        let r = &sent[0];
        assert_eq!(r.header("x-amz-target"), Some("TrentService.Sign"));
        assert!(r
            .header("authorization")
            .unwrap()
            .contains("/us-east-1/kms/aws4_request"));
        let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(body["MessageType"], "DIGEST");
        assert_eq!(body["SigningAlgorithm"], "ECDSA_SHA_256");
        assert_eq!(body["KeyId"], "alias/signing");
        assert_eq!(body["Message"], b64().encode([7; 32]));
    }

    #[test]
    fn the_public_key_is_read_and_a_refusal_says_so() {
        let aws = FakeAws::default();
        aws.answer("POST /", 200, br#"{"KeyId":"k","PublicKey":"AQID"}"#);
        assert_eq!(
            Kms::new(account(&aws), "k").public_key().unwrap(),
            [1, 2, 3]
        );
        assert_eq!(
            aws.sent.borrow()[0].header("x-amz-target"),
            Some("TrentService.GetPublicKey")
        );
        let aws = FakeAws::default();
        aws.answer(
            "POST /",
            400,
            br#"{"__type":"AccessDeniedException","message":"no"}"#,
        );
        let err = Kms::new(account(&aws), "k").sign_digest(&[0; 32]);
        assert!(matches!(err, Err(AwsError::Denied(m)) if m.contains("KMS Sign")));
    }
}
