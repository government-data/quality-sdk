//! IAM Roles Anywhere's certificate revocation lists (decision 0001):
//! revoking an installation is importing a CRL with its certificate's
//! serial into the installations' trust anchor, or updating the one already
//! imported. REST JSON; the CRL travels as base64 of its PEM, as
//! `tools/verify/roles-anywhere.sh` imported it.

use std::rc::Rc;

use base64::Engine;
use serde::Deserialize;
use serde_json::json;

use crate::client::{Account, AwsError};
use crate::http::Request;
use crate::sigv4::uri_encode;

/// A CRL as Roles Anywhere keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crl {
    pub crl_id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub trust_anchor_arn: String,
}

#[derive(Deserialize)]
struct One {
    crl: Crl,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    #[serde(default)]
    crls: Vec<Crl>,
    next_token: Option<String>,
}

/// Roles Anywhere in the account's region.
#[derive(Debug, Clone)]
pub struct RolesAnywhere {
    account: Rc<Account>,
}

impl RolesAnywhere {
    pub fn new(account: Rc<Account>) -> Self {
        Self { account }
    }

    fn url(&self, path: &str) -> String {
        format!(
            "https://rolesanywhere.{}.amazonaws.com{path}",
            self.account.region()
        )
    }

    /// `ListCrls`, every page.
    pub fn list_crls(&self) -> Result<Vec<Crl>, AwsError> {
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = token.as_deref().map_or(String::new(), |t| {
                format!("?nextToken={}", uri_encode(t, false))
            });
            let r = Request::new("GET", &self.url(&format!("/crls{query}")));
            let page: Page = self.call(r, "Roles Anywhere ListCrls")?;
            all.extend(page.crls);
            match page.next_token.filter(|t| !t.is_empty()) {
                Some(t) => token = Some(t),
                None => return Ok(all),
            }
        }
    }

    /// `ImportCrl`, enabled, for the trust anchor.
    pub fn import_crl(
        &self,
        name: &str,
        trust_anchor_arn: &str,
        pem: &str,
    ) -> Result<Crl, AwsError> {
        let mut r = Request::new("POST", &self.url("/crls"));
        r.body = json!({
            "name": name,
            "crlData": b64(pem),
            "trustAnchorArn": trust_anchor_arn,
            "enabled": true,
        })
        .to_string()
        .into_bytes();
        let one: One = self.call(r, "Roles Anywhere ImportCrl")?;
        Ok(one.crl)
    }

    /// `UpdateCrl`: the CRL's new content.
    pub fn update_crl(&self, crl_id: &str, pem: &str) -> Result<Crl, AwsError> {
        let path = format!("/crl/{}", uri_encode(crl_id, false));
        let mut r = Request::new("PATCH", &self.url(&path));
        r.body = json!({ "crlData": b64(pem) }).to_string().into_bytes();
        let one: One = self.call(r, "Roles Anywhere UpdateCrl")?;
        Ok(one.crl)
    }

    /// `EnableCrl`.
    pub fn enable_crl(&self, crl_id: &str) -> Result<Crl, AwsError> {
        let path = format!("/crl/{}/enable", uri_encode(crl_id, false));
        let one: One = self.call(
            Request::new("POST", &self.url(&path)),
            "Roles Anywhere EnableCrl",
        )?;
        Ok(one.crl)
    }

    fn call<T: for<'de> Deserialize<'de>>(
        &self,
        mut r: Request,
        what: &str,
    ) -> Result<T, AwsError> {
        r.headers
            .push(("content-type".into(), "application/json".into()));
        let answer = self.account.send("rolesanywhere", r, what)?;
        serde_json::from_slice(&answer.body)
            .map_err(|_| AwsError::Failed(format!("{what}: not its answer")))
    }
}

fn b64(pem: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(pem.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{account, FakeAws};

    const CRL: &str = r#"{"crl":{"crlId":"c-1","name":"installations","enabled":true,"trustAnchorArn":"arn:ta"}}"#;

    #[test]
    fn a_crl_is_imported_enabled_for_the_trust_anchor() {
        let aws = FakeAws::default();
        aws.answer(
            "POST https://rolesanywhere.us-east-1.amazonaws.com/crls",
            201,
            CRL.as_bytes(),
        );
        let ra = RolesAnywhere::new(account(&aws));
        let crl = ra
            .import_crl("installations", "arn:ta", "-----BEGIN X509 CRL-----")
            .unwrap();
        assert_eq!(crl.crl_id, "c-1");
        let sent = aws.sent.borrow();
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert_eq!(body["enabled"], true);
        assert_eq!(body["trustAnchorArn"], "arn:ta");
        assert_eq!(body["crlData"], b64("-----BEGIN X509 CRL-----"));
        assert!(sent[0]
            .header("authorization")
            .unwrap()
            .contains("/us-east-1/rolesanywhere/aws4_request"));
    }

    #[test]
    fn a_crl_is_updated_and_enabled_by_its_id() {
        let aws = FakeAws::default();
        aws.answer("PATCH /crl/c-1", 200, CRL.as_bytes());
        aws.answer("POST /crl/c-1/enable", 200, CRL.as_bytes());
        let ra = RolesAnywhere::new(account(&aws));
        ra.update_crl("c-1", "pem").unwrap();
        ra.enable_crl("c-1").unwrap();
        let sent = aws.sent.borrow();
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert_eq!(body, json!({ "crlData": b64("pem") }));
    }

    #[test]
    fn every_page_of_crls_is_listed() {
        let aws = FakeAws::default();
        aws.answer(
            "GET /crls?nextToken=a%2Bb",
            200,
            br#"{"crls":[{"crlId":"c-2","name":"two"}]}"#,
        );
        aws.answer(
            "GET /crls",
            200,
            br#"{"crls":[{"crlId":"c-1","name":"one","enabled":true}],"nextToken":"a+b"}"#,
        );
        let crls = RolesAnywhere::new(account(&aws)).list_crls().unwrap();
        let ids: Vec<_> = crls.iter().map(|c| c.crl_id.as_str()).collect();
        assert_eq!(ids, ["c-1", "c-2"]);
        assert!(!crls[1].enabled);
    }
}
