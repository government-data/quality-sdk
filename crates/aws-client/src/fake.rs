//! Fakes for the tests of this crate and of its users: fixed credentials and
//! an AWS that answers from a table. They call no account.

use std::cell::RefCell;
use std::rc::Rc;

use crate::client::{Account, Bucket};
use crate::credentials::{CredentialSource, Credentials};
use crate::http::{Request, Response, Transport};

/// Fixed credentials.
#[derive(Debug)]
pub struct Fixed;
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
#[derive(Debug, Default, Clone)]
pub struct FakeAws {
    pub answers: Rc<RefCell<Vec<(String, Response)>>>,
    pub sent: Rc<RefCell<Vec<Request>>>,
}

impl FakeAws {
    /// Answer `"GET <url suffix>"` with `status` and `body`.
    pub fn answer(&self, what: &str, status: u16, body: &[u8]) {
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

/// An account in `us-east-1` with [`Fixed`] credentials, sending to `aws`.
pub fn account(aws: &FakeAws) -> Rc<Account> {
    Rc::new(Account::new(
        "us-east-1",
        Box::new(Fixed),
        Box::new(aws.clone()),
    ))
}

/// The bucket `releases` of [`account`].
pub fn bucket(aws: &FakeAws) -> Bucket {
    Bucket::new(account(aws), "releases")
}
