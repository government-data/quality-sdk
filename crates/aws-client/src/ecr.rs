//! The installations' registry in ECR (decision 0001):
//! `GetAuthorizationToken`, the registry's host and a temporary credential
//! (12 hours, kept only in memory, which the agent pulls an offer's images
//! with), and `DescribeImages` of one digest, which `publish` checks every
//! image against before signing.

use std::rc::Rc;

use base64::Engine;
use serde::Deserialize;
use serde_json::json;

use crate::client::{check, Account, AwsError};
use crate::http::Request;

/// The registry's host and a temporary user and password for it.
#[derive(Clone, PartialEq, Eq)]
pub struct RegistryAuth {
    /// `<account>.dkr.ecr.<region>.amazonaws.com`.
    pub host: String,
    pub user: String,
    pub password: String,
}

impl std::fmt::Debug for RegistryAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryAuth")
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

/// The installation's registry in ECR.
#[derive(Debug, Clone)]
pub struct Ecr {
    account: Rc<Account>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Answer {
    authorization_data: Vec<Data>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Data {
    authorization_token: String,
    proxy_endpoint: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Described {
    #[serde(default)]
    image_details: Vec<Detail>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Detail {
    image_digest: String,
}

impl Ecr {
    pub fn new(account: Rc<Account>) -> Self {
        Self { account }
    }

    fn request(&self, action: &str, body: &serde_json::Value) -> Request {
        let url = format!("https://api.ecr.{}.amazonaws.com/", self.account.region());
        let mut request = Request::new("POST", &url);
        request.headers.extend([
            ("content-type".into(), "application/x-amz-json-1.1".into()),
            (
                "x-amz-target".into(),
                format!("AmazonEC2ContainerRegistry_V20150921.{action}"),
            ),
        ]);
        request.body = body.to_string().into_bytes();
        request
    }

    /// `DescribeImages` of `digest` (`sha256:<hex>`) in `repository` of the
    /// registry `registry_id` (its account): whether the image is there.
    pub fn has_image(
        &self,
        registry_id: &str,
        repository: &str,
        digest: &str,
    ) -> Result<bool, AwsError> {
        let body = json!({
            "registryId": registry_id,
            "repositoryName": repository,
            "imageIds": [{ "imageDigest": digest }],
        });
        let what = format!("ECR DescribeImages {repository}@{digest}");
        let request = self.request("DescribeImages", &body);
        let r = self.account.send_unchecked("ecr", request, &what)?;
        if r.status == 400 && String::from_utf8_lossy(&r.body).contains("ImageNotFoundException") {
            return Ok(false);
        }
        let r = check(&what, r)?;
        let answer: Described = serde_json::from_slice(&r.body)
            .map_err(|_| AwsError::Failed(format!("{what}: not its answer")))?;
        Ok(answer
            .image_details
            .iter()
            .any(|d| d.image_digest == digest))
    }

    /// `GetAuthorizationToken`: the registry's host and its credential.
    pub fn authorization(&self) -> Result<RegistryAuth, AwsError> {
        let request = self.request("GetAuthorizationToken", &json!({}));
        let what = "ECR GetAuthorizationToken";
        let r = self.account.send("ecr", request, what)?;
        let bad = |why: &str| AwsError::Failed(format!("{what}: {why}"));
        let answer: Answer = serde_json::from_slice(&r.body).map_err(|_| bad("not its answer"))?;
        let data = answer
            .authorization_data
            .into_iter()
            .next()
            .ok_or_else(|| bad("no authorization data"))?;
        let token = base64::engine::general_purpose::STANDARD
            .decode(data.authorization_token.trim())
            .ok()
            .and_then(|t| String::from_utf8(t).ok())
            .ok_or_else(|| bad("the token is not base64"))?;
        let (user, password) = token
            .split_once(':')
            .ok_or_else(|| bad("the token is not user:password"))?;
        let host = data
            .proxy_endpoint
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_string();
        Ok(RegistryAuth {
            host,
            user: user.into(),
            password: password.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{account, FakeAws};

    #[test]
    fn the_token_gives_the_registry_and_its_credential() {
        let aws = FakeAws::default();
        let token = base64::engine::general_purpose::STANDARD.encode("AWS:temporary");
        let body = format!(
            r#"{{"authorizationData":[{{"authorizationToken":"{token}","expiresAt":1.8E9,"proxyEndpoint":"https://1.dkr.ecr.us-east-1.amazonaws.com"}}]}}"#
        );
        aws.answer(
            "POST https://api.ecr.us-east-1.amazonaws.com/",
            200,
            body.as_bytes(),
        );
        let auth = Ecr::new(account(&aws)).authorization().unwrap();
        assert_eq!(auth.host, "1.dkr.ecr.us-east-1.amazonaws.com");
        assert_eq!(
            (auth.user.as_str(), auth.password.as_str()),
            ("AWS", "temporary")
        );
        assert!(!format!("{auth:?}").contains("temporary"), "never printed");
        let sent = aws.sent.borrow();
        let r = &sent[0];
        assert_eq!(
            r.header("x-amz-target"),
            Some("AmazonEC2ContainerRegistry_V20150921.GetAuthorizationToken")
        );
        assert!(r
            .header("authorization")
            .unwrap()
            .contains("/us-east-1/ecr/aws4_request"));
    }

    #[test]
    fn a_refusal_or_an_answer_that_is_not_a_token_fails() {
        let aws = FakeAws::default();
        aws.answer(
            "POST /",
            400,
            br#"{"__type":"AccessDeniedException","message":"no"}"#,
        );
        let err = Ecr::new(account(&aws)).authorization().unwrap_err();
        assert!(matches!(&err, AwsError::Denied(m) if m.contains("AccessDeniedException")));
        let aws = FakeAws::default();
        aws.answer("POST /", 200, br#"{"authorizationData":[]}"#);
        assert!(Ecr::new(account(&aws)).authorization().is_err());
    }

    #[test]
    fn an_image_is_looked_up_by_its_digest_in_the_registry() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let aws = FakeAws::default();
        let body = format!(r#"{{"imageDetails":[{{"imageDigest":"{digest}"}}]}}"#);
        aws.answer("POST /", 200, body.as_bytes());
        let ecr = Ecr::new(account(&aws));
        assert!(ecr.has_image("1", "quality-data/q/api", &digest).unwrap());
        let sent = aws.sent.borrow();
        assert_eq!(
            sent[0].header("x-amz-target"),
            Some("AmazonEC2ContainerRegistry_V20150921.DescribeImages")
        );
        let asked: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert_eq!(
            asked,
            json!({"registryId": "1", "repositoryName": "quality-data/q/api",
                   "imageIds": [{"imageDigest": digest}]})
        );

        let aws = FakeAws::default();
        aws.answer(
            "POST /",
            400,
            br#"{"__type":"ImageNotFoundException","message":"none"}"#,
        );
        assert!(!Ecr::new(account(&aws))
            .has_image("1", "r", &digest)
            .unwrap());
        let aws = FakeAws::default();
        aws.answer(
            "POST /",
            400,
            br#"{"__type":"RepositoryNotFoundException","message":"none"}"#,
        );
        let err = Ecr::new(account(&aws))
            .has_image("1", "r", &digest)
            .unwrap_err();
        assert!(matches!(&err, AwsError::Failed(m) if m.contains("RepositoryNotFound")));
    }
}
