//! Shared scaffolding for the integration tests.
//!
//! The keys below are generated for this test suite and are in the repository
//! on purpose: an OIDC test that cannot mint a signed id_token is testing a
//! stub rather than the verifier. They authenticate nothing anywhere.

#![allow(dead_code)]

use std::sync::Arc;

use brocanth_auth::accounts::Accounts;
use brocanth_auth::alerts::LoginAlerts;
use brocanth_auth::clock::TestClock;
use brocanth_auth::http_client::{HttpResponse, StubHttp};
use brocanth_auth::memory::MemoryStore;
use brocanth_auth::mfa::Mfa;
use brocanth_auth::oauth::{ClientConfig, OauthClient};
use brocanth_auth::otp::Otp;
use brocanth_auth::password::{PasswordCost, Passwords};
use brocanth_auth::session::{SessionConfig, Sessions, SigningKey};

/// A 2048-bit RSA private key, for signing test id_tokens. Test-only.
pub const TEST_RSA_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC+Gz5Bq5T1tHZs
Iik38SwOt5Ptn7bGZJSmLUdxXYE2PEEZ+U6zQOEPucXsu0Hj7HDRD12Mqdc17Tmi
i5R3GN0g7z1Xj81gl3EbO5g5KPh+PdsoAHzIcRwxnjdioB6o1qENHEp6sAyTHRMn
VzgjuYhAINiLBBQKuA6kMj9SGnb0LZmHz94BtKeLUBM8tFv6G7H+ZsZADy+X7aLR
g3E0gVAJtYbC9F1jWtlfaF46LCelpQscGEUl26e5h2RYfoZiKzDpSnIsYlnt7P3y
ZEKHmoCAmfQXUos7b64wMWfgZJcXvOwQ9OU0P6cuJsSXEkyWFMuyA9mGszxY/rlm
TizIC3F7AgMBAAECggEAUZOKLw7sUFU8neE408cbU0NmaIjpGxANE5QJfLWVnUrU
2sr3AQedrCxZKOEgAKi0rplL2PEGpx7hZQVFU6fpN9lBduH/zOuffwKfKyQQIcf0
U6GF6lNTOF1+wcUY8vWKzX/u29/QvY4z6mVMc1QJig8uLNU2rnTV35KqRlaR5ewv
cBpgPzVGhO9y+DSy9VzEtd/JfUfhXMPUvkfNm+4V7+JKMOaFBU1N/83E6MnYLTxN
MH/3pI7F6Y2ru9zdY83ooh43JdyQHpGT3CEMLCSk55gVj5Qy1wvoMUXbDo3kkj8D
12kmlbNdxBUKS5L1hq7+us/C9i4DF1vlT8jGbJqTpQKBgQDu3LllISJL53GrNGYh
LvsKqKpXN66AxnvTWlbZ33HyWMq5N9ZNVeAPgTzpHDqSlMFb5c864HTKYfNEWxOw
h0oQ5GwlHONKzE52Aadz+sYL4/wrXjffb/WFDKbJfoDRKKi0Oiga+z/rlXHQ7eQl
Liq3GZSWZ47JsPnNvGs+YAV5PQKBgQDLvwCaf56iyZi5ObvHUioWmZcP/e23AB0y
md/gWHZ5AUf/wjGDFbEx0s+wGXsbTZfZBipOltDHVkyDat8MAw61hbyfNvXpuvZr
4zPA3v7bZFZCmPasgwF2IEcRDwL7Vefx2eB5q9kJPu88LdrKwQPAdTg5CA6s7jjP
kxd3r4qRFwKBgBAZPkGLnyUgfy+7w1pfMTpBreFc9lzlg7j18L6UabictWUQV1g0
/nntCu7nISd58SJBddsQ/64YNe4YeVY/eAV1b7h8nvEOngckC2Yss6ZfgkJmAjwD
7rc96AvSg9oU9HHZLZTL9D3rf5VGQ0ciOmzGw5XzetMrI1rzZQL6M8Q9AoGBAIJE
lvAxVI4kcx9KiIMCR6K3hNxWsJxm2EQ4Geno5aasaQpfcSSsw+RlMyEWyZ1FWScL
SBWDW+13gegsdnvJoKfy4Ay0KTrNrSyB0d8pNK21U2m1P1JBWLfcH1QK0w87d9cs
RNfGRjghIvjJILCuA+YTjGT6f9eVYIFnPIPrmp71AoGBAMYTwC1zBqRKMmzXdeLC
gH2YLInn5NB+dmvxC55Agr10Jf6MDsHL/KYeAepLMejyWdffEzZS39N1CChVH0J1
ArQyve5KaEZ85uEDPakHeSGM/Z5/eyPstLB/fqnuLs46YBSDKOssr0cRjzA5AI1O
HIzldEiH+oyRzSI/CProm4Me
-----END PRIVATE KEY-----"#;

/// The JWKS that matches [`TEST_RSA_PEM`], served by the stub.
pub const TEST_JWKS: &str = r#"{"keys": [{"kty": "RSA", "use": "sig", "alg": "RS256", "kid": "test-key-1", "n": "vhs-QauU9bR2bCIpN_EsDreT7Z-2xmSUpi1HcV2BNjxBGflOs0DhD7nF7LtB4-xw0Q9djKnXNe05oouUdxjdIO89V4_NYJdxGzuYOSj4fj3bKAB8yHEcMZ43YqAeqNahDRxKerAMkx0TJ1c4I7mIQCDYiwQUCrgOpDI_Uhp29C2Zh8_eAbSni1ATPLRb-hux_mbGQA8vl-2i0YNxNIFQCbWGwvRdY1rZX2heOiwnpaULHBhFJdunuYdkWH6GYisw6UpyLGJZ7ez98mRCh5qAgJn0F1KLO2-uMDFn4GSXF7zsEPTlND-nLibElxJMlhTLsgPZhrM8WP65Zk4syAtxew", "e": "AQAB"}]}"#;

/// A P-256 private key, for the Apple client-secret test. Test-only.
pub const TEST_EC_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgzREeTyW9wdrcJ6HA
AMPtVeETAWWmQNdEYZ+zGNobPtehRANCAATWjoAIA76MYBRGiXTsSsTxCJgGVu7d
fJgXKQxK/6iK3YlApXC+bNX0B6Fag4ffVi/yOcFny3pGL/3ENIhMlAD2
-----END PRIVATE KEY-----"#;

/// The `kid` in [`TEST_JWKS`].
pub const TEST_KID: &str = "test-key-1";

/// Signs a set of claims as an RS256 id_token the stub's JWKS can verify.
pub fn sign_id_token(claims: &serde_json::Value) -> String {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(TEST_KID.to_string());
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(TEST_RSA_PEM.as_bytes()).expect("rsa key");
    jsonwebtoken::encode(&header, claims, &key).expect("sign")
}

/// Signs claims with HS256 — the algorithm-confusion attack, for the test that
/// proves it is refused.
pub fn sign_hs256(claims: &serde_json::Value) -> String {
    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    let key = jsonwebtoken::EncodingKey::from_secret(b"anything at all");
    jsonwebtoken::encode(&header, claims, &key).expect("sign")
}

/// Seconds since the epoch, from the real clock, because `jsonwebtoken`
/// validates `exp` against the system clock and takes no injected time.
pub fn real_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64
}

/// A store, a clock and a hasher cheap enough for a test.
pub struct Fixture {
    pub store: Arc<MemoryStore>,
    pub clock: Arc<TestClock>,
    pub passwords: Arc<Passwords>,
}

impl Fixture {
    pub fn new() -> Self {
        Self {
            store: Arc::new(MemoryStore::new()),
            clock: Arc::new(TestClock::default()),
            passwords: Arc::new(
                Passwords::new(PasswordCost::insecure_for_tests()).expect("hasher"),
            ),
        }
    }

    /// A hasher at a cost where argon2 actually dominates, for the timing
    /// tests. Still far below production cost, and still real argon2id.
    pub fn with_measurable_cost() -> Self {
        let mut f = Self::new();
        f.passwords = Arc::new(
            Passwords::new(PasswordCost::custom(32 * 1024, 1, 1).expect("cost")).expect("hasher"),
        );
        f
    }

    pub fn sessions(&self) -> Sessions<MemoryStore, TestClock> {
        Sessions::new(
            self.store.clone(),
            self.clock.clone(),
            SigningKey::from_bytes([9u8; 32]),
            SessionConfig::default(),
        )
    }

    pub fn accounts(&self) -> Accounts<MemoryStore, TestClock> {
        Accounts::new(
            self.store.clone(),
            self.clock.clone(),
            self.passwords.clone(),
        )
        .expect("accounts")
    }

    pub fn mfa(&self) -> Mfa<MemoryStore, TestClock> {
        Mfa::new(self.store.clone(), self.clock.clone()).expect("mfa")
    }

    pub fn otp(&self) -> Otp<MemoryStore, TestClock> {
        Otp::new(
            self.store.clone(),
            self.clock.clone(),
            self.passwords.clone(),
        )
        .expect("otp")
    }

    pub fn alerts(&self) -> LoginAlerts<MemoryStore, TestClock> {
        LoginAlerts::new(
            self.store.clone(),
            self.clock.clone(),
            SigningKey::from_bytes([9u8; 32]),
        )
    }
}

/// An OAuth client over a stubbed provider.
pub fn oauth_client(
    http: Arc<StubHttp>,
    clock: Arc<TestClock>,
) -> OauthClient<StubHttp, TestClock> {
    OauthClient::new(http, clock)
}

/// The usual registration at a provider.
pub fn client_config() -> ClientConfig {
    ClientConfig::new(
        "client-id-123",
        "client-secret-456",
        "https://app.test/auth/callback",
    )
}

/// A `200 OK` JSON response.
pub fn json(body: impl Into<Vec<u8>>) -> HttpResponse {
    HttpResponse::json(body)
}
