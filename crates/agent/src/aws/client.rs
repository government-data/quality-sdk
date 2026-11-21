//! The S3 calls the installation role may make (decision 0001): read
//! `index/` and `packages/`, write its own `reports/<id>/`. Signed with
//! SigV4 and the Roles Anywhere credentials.

use chrono::Utc;

use super::credentials::CredentialSource;
use super::http::{Request, Response, Transport};
use super::sigv4::{payload_hash, sign, uri_encode};

/// Why an AWS call did not do what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AwsError {
    /// AWS said no (403): a revoked certificate, or an object the role may
    /// not read. Without `s3:ListBucket`, a missing object is also a 403.
    Denied(String),
    /// No answer, or one that says to try later (5xx, throttling).
    Unavailable(String),
    /// Any other answer.
    Failed(String),
}

impl std::fmt::Display for AwsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied(m) => write!(f, "denied: {m}"),
            Self::Unavailable(m) => write!(f, "unavailable: {m}"),
            Self::Failed(m) => write!(f, "{m}"),
        }
    }
}

/// The release bucket, as the installation role sees it.
pub struct Bucket {
    region: String,
    bucket: String,
    credentials: Box<dyn CredentialSource>,
    transport: Box<dyn Transport>,
}

impl std::fmt::Debug for Bucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bucket")
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .finish_non_exhaustive()
    }
}

impl Bucket {
    pub fn new(
        region: &str,
        bucket: &str,
        credentials: Box<dyn CredentialSource>,
        transport: Box<dyn Transport>,
    ) -> Self {
        Self {
            region: region.into(),
            bucket: bucket.into(),
            credentials,
            transport,
        }
    }

    fn url(&self, key: &str) -> String {
        format!(
            "https://{}.s3.{}.amazonaws.com/{}",
            self.bucket,
            self.region,
            uri_encode(key, true)
        )
    }

    /// `GetObject`.
    pub fn get(&self, key: &str) -> Result<Vec<u8>, AwsError> {
        let request = Request::new("GET", &self.url(key));
        let r = self.send(request)?;
        Ok(r.body)
    }

    /// `PutObject`.
    pub fn put(&self, key: &str, body: Vec<u8>, content_type: &str) -> Result<(), AwsError> {
        let mut request = Request::new("PUT", &self.url(key));
        request
            .headers
            .push(("content-type".into(), content_type.into()));
        request.body = body;
        self.send(request).map(|_| ())
    }

    fn send(&self, mut request: Request) -> Result<Response, AwsError> {
        let credentials = self.credentials.credentials().map_err(|e| {
            if e.contains("AccessDenied") {
                AwsError::Denied(e)
            } else {
                AwsError::Unavailable(e)
            }
        })?;
        request
            .headers
            .push(("x-amz-content-sha256".into(), payload_hash(&request.body)));
        sign(&mut request, &credentials, &self.region, "s3", Utc::now())
            .map_err(AwsError::Failed)?;
        let what = format!(
            "{} s3://{}{}",
            request.method,
            self.bucket,
            path_of(&request.url)
        );
        let r = self
            .transport
            .send(request)
            .map_err(|e| AwsError::Unavailable(format!("{what}: {e}")))?;
        check(&what, r)
    }
}

fn path_of(url: &str) -> &str {
    url.find(".amazonaws.com")
        .map_or(url, |i| &url[i + ".amazonaws.com".len()..])
}

/// A 2xx answer, or the error its status and S3 code say.
pub(crate) fn check(what: &str, r: Response) -> Result<Response, AwsError> {
    if (200..300).contains(&r.status) {
        return Ok(r);
    }
    let code = error_code(&r.body).unwrap_or_default();
    let m = format!("{what}: {} {code}", r.status);
    Err(match r.status {
        403 => AwsError::Denied(m),
        429 | 500..=599 => AwsError::Unavailable(m),
        _ if code.contains("Throttl") || code == "SlowDown" => AwsError::Unavailable(m),
        _ => AwsError::Failed(m),
    })
}

/// The `<Code>` of an S3 XML error, or the `__type` of a JSON one.
fn error_code(body: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    if let Some(start) = text.find("<Code>") {
        let rest = &text[start + 6..];
        return rest.find("</Code>").map(|end| rest[..end].to_string());
    }
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let t = json.get("__type")?.as_str()?;
    Some(t.rsplit('#').next().unwrap_or(t).to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::super::credentials::Credentials;
    use super::*;

    /// Fixed credentials.
    pub(crate) struct Fixed;
    impl CredentialSource for Fixed {
        fn credentials(&self) -> Result<Credentials, String> {
            Ok(Credentials {
                access_key_id: "ASIAEXAMPLE".into(),
                secret_access_key: "secret".into(),
                session_token: Some("token".into()),
                expiration: None,
            })
        }
    }

    /// A fake AWS: answers by method and URL, and keeps what it was sent.
    #[derive(Default, Clone)]
    pub(crate) struct FakeAws {
        pub(crate) answers: Rc<RefCell<Vec<(String, Response)>>>,
        pub(crate) sent: Rc<RefCell<Vec<Request>>>,
    }

    impl FakeAws {
        /// Answer `"GET <url suffix>"` with `status` and `body`.
        pub(crate) fn answer(&self, what: &str, status: u16, body: &[u8]) {
            let r = Response {
                status,
                body: body.to_vec(),
            };
            self.answers.borrow_mut().push((what.into(), r));
        }
    }

    impl Transport for FakeAws {
        fn send(&self, r: Request) -> Result<Response, String> {
            let found = self.answers.borrow().iter().find_map(|(what, answer)| {
                let (method, suffix) = what.split_once(' ').unwrap();
                (r.method == method && r.url.ends_with(suffix)).then(|| answer.clone())
            });
            self.sent.borrow_mut().push(r);
            found.ok_or_else(|| "connection refused".into())
        }
    }

    pub(crate) fn bucket(aws: &FakeAws) -> Bucket {
        Bucket::new(
            "us-east-1",
            "releases",
            Box::new(Fixed),
            Box::new(aws.clone()),
        )
    }

    #[test]
    fn an_object_is_read_with_a_signed_request() {
        let aws = FakeAws::default();
        aws.answer("GET /index/stable.json", 200, b"{}");
        assert_eq!(bucket(&aws).get("index/stable.json").unwrap(), b"{}");
        let sent = aws.sent.borrow();
        let r = &sent[0];
        assert_eq!(
            r.url,
            "https://releases.s3.us-east-1.amazonaws.com/index/stable.json"
        );
        let auth = r.header("authorization").unwrap();
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=ASIAEXAMPLE/"),
            "{auth}"
        );
        assert!(auth.contains("/us-east-1/s3/aws4_request"), "{auth}");
        assert_eq!(r.header("x-amz-security-token"), Some("token"));
        assert_eq!(
            r.header("x-amz-content-sha256"),
            Some(payload_hash(b"").as_str())
        );
    }

    #[test]
    fn an_object_is_written_with_its_hash_signed() {
        let aws = FakeAws::default();
        aws.answer("PUT /reports/inst-1/r.json", 200, b"");
        bucket(&aws)
            .put(
                "reports/inst-1/r.json",
                b"{\"a\":1}".to_vec(),
                "application/json",
            )
            .unwrap();
        let r = &aws.sent.borrow()[0];
        assert_eq!(
            r.header("x-amz-content-sha256"),
            Some(payload_hash(b"{\"a\":1}").as_str())
        );
        assert_eq!(r.header("content-type"), Some("application/json"));
        assert!(r
            .header("authorization")
            .unwrap()
            .contains("content-type;host;"));
    }

    #[test]
    fn answers_become_errors_by_status() {
        let aws = FakeAws::default();
        let denied = b"<Error><Code>AccessDenied</Code><Message>no</Message></Error>";
        aws.answer("GET /index/a.json", 403, denied);
        aws.answer(
            "GET /index/b.json",
            503,
            b"<Error><Code>SlowDown</Code></Error>",
        );
        aws.answer(
            "GET /index/c.json",
            400,
            b"<Error><Code>BadDigest</Code></Error>",
        );
        let b = bucket(&aws);
        let err = b.get("index/a.json").unwrap_err();
        assert_eq!(
            err,
            AwsError::Denied("GET s3://releases/index/a.json: 403 AccessDenied".into())
        );
        assert!(matches!(
            b.get("index/b.json"),
            Err(AwsError::Unavailable(_))
        ));
        assert!(
            matches!(b.get("index/c.json"), Err(AwsError::Failed(m)) if m.contains("BadDigest"))
        );
        // No answer at all.
        assert!(matches!(
            b.get("index/d.json"),
            Err(AwsError::Unavailable(_))
        ));
        assert_eq!(
            error_code(br#"{"__type":"com.amazon#AccessDeniedException"}"#).unwrap(),
            "AccessDeniedException"
        );
    }
}
