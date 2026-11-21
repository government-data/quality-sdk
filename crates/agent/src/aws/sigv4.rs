//! AWS Signature Version 4, for the few calls the agent makes (S3
//! `GetObject` and `PutObject`, ECR `GetAuthorizationToken`). Checked against
//! the examples AWS publishes for SigV4 and for S3 (see the tests).

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use super::credentials::Credentials;
use super::http::Request;

/// The hex SHA-256 of a payload, as `x-amz-content-sha256` carries it.
pub fn payload_hash(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

/// Sign `request` for `service` in `region` at `now`: adds `x-amz-date`,
/// `x-amz-security-token` (with temporary credentials) and `authorization`.
/// Every header already in the request is signed, and the URL's host.
pub fn sign(
    request: &mut Request,
    credentials: &Credentials,
    region: &str,
    service: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let (host, path, query) = split_url(&request.url)?;
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = &amz_date[..8];
    request
        .headers
        .push(("x-amz-date".into(), amz_date.clone()));
    if let Some(token) = &credentials.session_token {
        request
            .headers
            .push(("x-amz-security-token".into(), token.clone()));
    }

    let mut headers: Vec<(String, String)> = request
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.to_ascii_lowercase(),
                v.split_whitespace().collect::<Vec<_>>().join(" "),
            )
        })
        .chain(std::iter::once(("host".to_string(), host.to_string())))
        .collect();
    headers.sort();
    let signed: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
    let signed = signed.join(";");
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let payload = request
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-amz-content-sha256"))
        .map_or_else(|| payload_hash(&request.body), |(_, v)| v.clone());
    let canonical = format!(
        "{}\n{}\n{}\n{canonical_headers}\n{signed}\n{payload}",
        request.method,
        canonical_path(path, service),
        canonical_query(query),
    );

    let scope = format!("{date}/{region}/{service}/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        payload_hash(canonical.as_bytes())
    );
    let mut key = hmac(
        format!("AWS4{}", credentials.secret_access_key).as_bytes(),
        date,
    );
    for part in [region, service, "aws4_request"] {
        key = hmac(&key, part);
    }
    let signature = hex::encode(hmac(&key, &to_sign));
    request.headers.push((
        "authorization".into(),
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}",
            credentials.access_key_id
        ),
    ));
    Ok(())
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    // HMAC takes a key of any length.
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// `https://host/path?query` into its host, path and query.
fn split_url(url: &str) -> Result<(&str, &str, &str), String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| format!("{url} is not an HTTP URL"))?;
    let (host, path) = rest.find('/').map_or((rest, "/"), |i| rest.split_at(i));
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    Ok((host, path, query))
}

/// Every byte but the unreserved ones percent-encoded (and `/` kept when
/// it separates a path).
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'.' | b'_' | b'~')
            || (keep_slash && b == b'/')
        {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// S3 signs the path as sent (each segment encoded once); every other
/// service encodes the already encoded path again.
fn canonical_path(path: &str, service: &str) -> String {
    if service == "s3" {
        path.to_string()
    } else {
        uri_encode(path, true)
    }
}

/// The query's parameters sorted by name, each already encoded as sent.
fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<(&str, &str)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| p.split_once('=').unwrap_or((p, "")))
        .collect();
    pairs.sort_unstable();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn credentials(access: &str, secret: &str) -> Credentials {
        Credentials {
            access_key_id: access.into(),
            secret_access_key: secret.into(),
            session_token: None,
            expiration: None,
        }
    }

    fn signature(r: &Request) -> &str {
        let (_, auth) = r
            .headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .unwrap();
        auth.rsplit("Signature=").next().unwrap()
    }

    /// AWS's SigV4 test suite, `get-vanilla`.
    #[test]
    fn the_sigv4_suite_get_vanilla() {
        let mut r = Request::new("GET", "https://example.amazonaws.com/");
        let c = credentials("AKIDEXAMPLE", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY");
        let now = Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap();
        sign(&mut r, &c, "us-east-1", "service", now).unwrap();
        assert_eq!(
            signature(&r),
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    /// The S3 documentation's "GET Object" example (header-based auth).
    #[test]
    fn the_s3_get_object_example() {
        let mut r = Request::new("GET", "https://examplebucket.s3.amazonaws.com/test.txt");
        r.headers.push(("Range".into(), "bytes=0-9".into()));
        r.headers
            .push(("x-amz-content-sha256".into(), payload_hash(b"")));
        let c = credentials(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        sign(&mut r, &c, "us-east-1", "s3", now).unwrap();
        assert_eq!(
            signature(&r),
            "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    /// The S3 documentation's "PUT Object" example: an encoded key and a
    /// body.
    #[test]
    fn the_s3_put_object_example() {
        let key = uri_encode("test$file.text", true);
        let mut r = Request::new(
            "PUT",
            &format!("https://examplebucket.s3.amazonaws.com/{key}"),
        );
        r.body = b"Welcome to Amazon S3.".to_vec();
        r.headers
            .push(("Date".into(), "Fri, 24 May 2013 00:00:00 GMT".into()));
        r.headers
            .push(("x-amz-storage-class".into(), "REDUCED_REDUNDANCY".into()));
        r.headers
            .push(("x-amz-content-sha256".into(), payload_hash(&r.body)));
        let c = credentials(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        sign(&mut r, &c, "us-east-1", "s3", now).unwrap();
        assert_eq!(
            signature(&r),
            "98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
        );
    }

    #[test]
    fn temporary_credentials_sign_their_token() {
        let mut r = Request::new("GET", "https://example.amazonaws.com/");
        let mut c = credentials("ASIAEXAMPLE", "secret");
        c.session_token = Some("token".into());
        sign(&mut r, &c, "us-east-1", "s3", Utc::now()).unwrap();
        let auth = &r.headers.last().unwrap().1;
        assert!(auth.contains("SignedHeaders=host;x-amz-date;x-amz-security-token"));
        assert!(r
            .headers
            .contains(&("x-amz-security-token".into(), "token".into())));
    }

    #[test]
    fn encoding_and_queries() {
        assert_eq!(uri_encode("a b/c~d", true), "a%20b/c~d");
        assert_eq!(uri_encode("a/b", false), "a%2Fb");
        assert_eq!(canonical_query("b=2&a=1"), "a=1&b=2");
        assert!(split_url("ftp://x").is_err());
    }
}
