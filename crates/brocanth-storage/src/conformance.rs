//! The conformance suites: what every backend must do, run against every
//! backend.
//!
//! [`storage`] holds a [`Storage`] to the content-addressed contract;
//! [`bucket`] holds a [`Bucket`] to the raw one. A backend is correct when
//! both pass on it, so a new backend adds a runner and no case.
//! `docs/object-storage.md` §7 has the table of who runs what, and when.
//!
//! Each run works under its own addresses and its own prefix, so one
//! server serves parallel runs with no cleanup between them.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;

use crate::bucket::{
    Body, Bucket, ByteRange, ByteStream, ObjectPath, PutOptions, SignedGet, SignedMethod,
};
use crate::error::StorageError;
use crate::key::ObjectKey;
use crate::meta::{ContentType, ObjectInfo};
use crate::spec::VariantSpec;
use crate::storage::Storage;

/// Reads and writes a signed URL, for the backends that sign. A test
/// passes one built on `reqwest`; the disk passes none.
pub trait Fetch: Sync {
    /// `GET url`, answering the body, or an error with the status.
    fn get<'a>(&'a self, url: &'a str) -> crate::conformance::Fut<'a, Result<Vec<u8>, String>>;
    /// `PUT url` with `body`.
    fn put<'a>(
        &'a self,
        url: &'a str,
        body: Vec<u8>,
    ) -> crate::conformance::Fut<'a, Result<(), String>>;
    /// `method url`, answering the status, the headers and the body
    /// whatever the status.
    fn send<'a>(
        &'a self,
        method: &'a str,
        url: &'a str,
    ) -> crate::conformance::Fut<'a, Result<Fetched, String>>;
}

/// What [`Fetch::send`] answers.
#[derive(Debug)]
pub struct Fetched {
    /// The status code.
    pub status: u16,
    /// The headers, names lowercased.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

impl Fetched {
    /// The first value of a header.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A boxed future, for [`Fetch`].
pub type Fut<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// What a backend can do beyond the common contract.
pub struct Can<'a> {
    /// The store refuses an object past this many bytes; the suite stores
    /// one byte past it.
    pub limit: Option<u64>,
    /// The store signs URLs. With a fetcher, the suite reads (and for a
    /// bucket, writes) through them; without one it checks only that a URL
    /// comes back.
    pub signs: bool,
    /// Reads a signed URL back.
    pub fetch: Option<&'a dyn Fetch>,
    /// Puts a body large enough to go up in parts.
    pub big: bool,
    /// The server honours a create-only put. Every real service does; the
    /// GCS emulator ignores `ifGenerationMatch`, so its run says `false`
    /// and the suite checks only that the put succeeds on a free path.
    pub create_only: bool,
    /// The server checks a signed URL: its signature, its expiry and its
    /// method. MinIO does; the GCS emulator checks none of them, so its run
    /// says `false` and the suite reads the URL instead.
    pub verifies: bool,
    /// The server keeps a stored `Content-Disposition` and answers it on a
    /// `head`. S3 does, and real GCS; the emulator drops it from an
    /// XML-API put, and a disk keeps none.
    pub disposition: bool,
}

/// The query of a URL as decoded pairs.
fn query_pairs(url: &str) -> Vec<(String, String)> {
    let query = url.split_once('?').map_or("", |(_, q)| q);
    let decode = |s: &str| {
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < bytes.len() => {
                    let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz");
                    match u8::from_str_radix(hex, 16) {
                        Ok(b) => {
                            out.push(b);
                            i += 2;
                        }
                        Err(_) => out.push(b'%'),
                    }
                }
                b => out.push(b),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(k), decode(v))
        })
        .collect()
}

/// The decoded value of one query parameter, by name, any case.
fn query_value(url: &str, name: &str) -> Option<String> {
    query_pairs(url)
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

/// The URL with one query parameter taken out: what a bearer who wants the
/// object rendered would try.
fn without_param(url: &str, name: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| {
            !pair
                .split('=')
                .next()
                .unwrap_or("")
                .eq_ignore_ascii_case(name)
        })
        .collect();
    format!("{base}?{}", kept.join("&"))
}

/// A PNG header and some bytes no other run stores.
fn unique_png(what: &str) -> Vec<u8> {
    let key = ObjectKey::random().expect("randomness");
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend_from_slice(format!("conformance {what} {key}").as_bytes());
    out
}

/// The `Storage` contract.
pub async fn storage(store: Arc<dyn Storage>, can: Can<'_>) {
    // put, get, meta: the type from the bytes, the name defanged.
    let body = unique_png("round trip");
    let about = ObjectInfo::new()
        .with_filename("../ünï cødé%41.png")
        .with_declared_type("text/html");
    let key = store.put(&body, &about).await.expect("put");
    assert_eq!(store.get(&key).await.expect("get"), body);
    let meta = store.meta(&key).await.expect("meta");
    assert_eq!(meta.key, key);
    assert_eq!(meta.size, body.len() as u64);
    assert_eq!(
        meta.content_type.as_str(),
        "image/png",
        "sniffed, not declared"
    );
    assert_eq!(
        meta.filename.as_ref().map(|f| f.as_str()),
        Some("ünï cødé%41.png")
    );
    assert!(store.exists(&key).await.expect("exists"));

    // A second put of the same bytes: same address, the first metadata.
    let again = store
        .put(
            &body,
            &ObjectInfo::new()
                .with_filename("other.png")
                .with_declared_type("application/pdf"),
        )
        .await
        .expect("second put");
    assert_eq!(again, key);
    assert_eq!(
        store.meta(&key).await.expect("meta").filename,
        meta.filename
    );

    // A declared type is believed only when the bytes say nothing.
    let text = format!("plain {}", ObjectKey::random().unwrap()).into_bytes();
    let k = store
        .put(&text, &ObjectInfo::new().with_declared_type("text/html"))
        .await
        .expect("put text");
    let ct = store.meta(&k).await.expect("meta").content_type;
    assert!(!ct.is_safe_to_inline(), "{ct} must not be served inline");
    store.delete(&k).await.expect("delete");

    // put_at a random address.
    let random = ObjectKey::random().unwrap();
    store
        .put_at(&random, b"at a random address", &ObjectInfo::new())
        .await
        .expect("put_at");
    assert_eq!(store.get(&random).await.unwrap(), b"at a random address");

    // Ranges.
    let r = |s, e| ByteRange::new(s, e).unwrap();
    assert_eq!(store.get_range(&random, r(3, 9)).await.unwrap(), b"a rand");
    assert_eq!(
        store.get_range(&random, ByteRange::from(12)).await.unwrap(),
        b"address"
    );
    assert_eq!(
        store.get_range(&random, r(12, 500)).await.unwrap(),
        b"address"
    );
    assert!(matches!(
        store.get_range(&random, ByteRange::from(19)).await,
        Err(StorageError::InvalidRange)
    ));

    // Missing.
    let missing = ObjectKey::random().unwrap();
    assert!(!store.exists(&missing).await.unwrap());
    assert!(matches!(
        store.get(&missing).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.meta(&missing).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.get_range(&missing, r(0, 1)).await,
        Err(StorageError::NotFound)
    ));
    assert_eq!(
        store
            .get_variant(&missing, VariantSpec::parse("10x10").unwrap())
            .await
            .unwrap(),
        None
    );

    // Variants: refused for a missing object, kept, and gone with it.
    let spec = VariantSpec::parse("300x200t").unwrap();
    let other = VariantSpec::parse("150x150").unwrap();
    assert!(matches!(
        store.put_variant(&missing, spec, b"x").await,
        Err(StorageError::NotFound)
    ));
    assert_eq!(store.get_variant(&key, spec).await.unwrap(), None);
    store
        .put_variant(&key, spec, b"small")
        .await
        .expect("put variant");
    store
        .put_variant(&key, spec, b"small")
        .await
        .expect("again");
    store
        .put_variant(&key, other, b"smaller")
        .await
        .expect("other");
    assert_eq!(
        store.get_variant(&key, spec).await.unwrap().unwrap(),
        b"small"
    );
    store.delete(&key).await.expect("delete");
    assert!(!store.exists(&key).await.unwrap());
    assert_eq!(store.get_variant(&key, spec).await.unwrap(), None);
    assert_eq!(store.get_variant(&key, other).await.unwrap(), None);
    store.delete(&key).await.expect("delete is idempotent");

    // The limit, refused before anything is stored.
    if let Some(limit) = can.limit {
        let mut big = unique_png("too large");
        big.resize(limit as usize + 1, b'x');
        match store.put(&big, &ObjectInfo::new()).await {
            Err(StorageError::TooLarge { size, limit: l }) => {
                assert_eq!(l, limit);
                assert_eq!(size, limit + 1);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    // Signed URLs carry the download rule (D29): a non-image as an
    // attachment of type `octet-stream`, a sniffed image inline as its type.
    assert!(!can.signs || store.can_sign());
    let url = store
        .signed_url(&random, Duration::from_secs(60))
        .await
        .expect("signed url");
    let png = unique_png("signed");
    let png_key = store
        .put(&png, &ObjectInfo::new().with_filename("p.png"))
        .await
        .expect("put png");
    let png_url = store
        .signed_url(&png_key, Duration::from_secs(60))
        .await
        .expect("signed url of a png");
    if can.signs {
        let url = url.expect("this backend signs");
        assert!(url.starts_with("http"), "a signed URL is a URL");
        assert_eq!(
            query_value(&url, "response-content-type").as_deref(),
            Some("application/octet-stream")
        );
        assert_eq!(
            query_value(&url, "response-content-disposition").as_deref(),
            Some("attachment; filename=\"download\"")
        );
        let png_url = png_url.expect("this backend signs");
        assert_eq!(
            query_value(&png_url, "response-content-type").as_deref(),
            Some("image/png")
        );
        assert_eq!(query_value(&png_url, "response-content-disposition"), None);
        if let Some(fetch) = can.fetch {
            assert_eq!(
                fetch.get(&url).await.expect("fetch"),
                b"at a random address"
            );
            if can.verifies {
                let got = fetch.send("GET", &url).await.expect("fetch");
                assert_eq!(got.status, 200);
                assert_eq!(got.header("content-type"), Some("application/octet-stream"));
                assert_eq!(
                    got.header("content-disposition"),
                    Some("attachment; filename=\"download\"")
                );
                let got = fetch.send("GET", &png_url).await.expect("fetch");
                assert_eq!(got.status, 200);
                assert_eq!(got.header("content-type"), Some("image/png"));
                assert_eq!(got.body, png);
            }
        }
        assert!(matches!(
            store.signed_url(&missing, Duration::from_secs(60)).await,
            Err(StorageError::NotFound)
        ));
        let origin = store
            .signing_origin()
            .await
            .expect("signs a probe")
            .expect("an origin");
        assert!(url.starts_with(&origin), "{url} is not under {origin}");
    } else {
        assert_eq!(url, None);
        assert_eq!(png_url, None);
        assert_eq!(store.signing_origin().await.expect("answers"), None);
    }
    store.delete(&png_key).await.unwrap();
    store.delete(&random).await.unwrap();
}

/// What a cloud put stores beside the bytes (D31): a non-image carries
/// `Content-Disposition: attachment`, a sniffed image none. Read back with
/// the bucket's `head`.
#[cfg(any(feature = "s3", feature = "gcs"))]
pub async fn stored_disposition(store: &crate::objects::ObjectStorage) {
    let text = format!("<html>{}</html>", ObjectKey::random().unwrap()).into_bytes();
    let k = store
        .put(
            &text,
            &ObjectInfo::new()
                .with_filename("page.html")
                .with_declared_type("text/html"),
        )
        .await
        .expect("put html");
    let head = store
        .bucket()
        .head(&crate::objects::object_path(&k))
        .await
        .expect("head");
    assert_eq!(
        head.content_disposition.as_deref(),
        Some("attachment; filename=\"page.html\"")
    );
    let png = unique_png("disposition");
    let p = store.put(&png, &ObjectInfo::new()).await.expect("put png");
    let head = store
        .bucket()
        .head(&crate::objects::object_path(&p))
        .await
        .expect("head");
    assert_eq!(head.content_disposition, None);
    store.delete(&k).await.unwrap();
    store.delete(&p).await.unwrap();
}

fn path(base: &str, rest: &str) -> ObjectPath {
    ObjectPath::parse(&format!("{base}/{rest}")).expect("a test path")
}

fn chunks(parts: Vec<Vec<u8>>) -> Body {
    struct Chunks(std::vec::IntoIter<Vec<u8>>);
    impl futures_core::Stream for Chunks {
        type Item = Result<Bytes, StorageError>;
        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            std::task::Poll::Ready(self.0.next().map(|c| Ok(Bytes::from(c))))
        }
    }
    let stream: ByteStream = Box::pin(Chunks(parts.into_iter()));
    Body::Stream(stream)
}

/// The `Bucket` contract, under `base`, a prefix no other run uses.
pub async fn bucket(b: Arc<dyn Bucket>, base: &str, can: Can<'_>) {
    b.probe().await.expect("probe");

    // put with a type and metadata; head and get answer them.
    let p = path(base, "one/a.bin");
    let options = PutOptions::new()
        .with_content_type("application/x-test")
        .with_metadata("filename", "ünï cødé %41.png");
    let head = b
        .put(&p, b"0123456789".as_slice().into(), options)
        .await
        .expect("put");
    assert_eq!(head.size, 10);
    let head = b.head(&p).await.expect("head");
    assert_eq!(head.size, 10);
    assert_eq!(head.path, p);
    assert_eq!(head.content_type.as_deref(), Some("application/x-test"));
    assert_eq!(
        head.metadata.get("filename").map(String::as_str),
        Some("ünï cødé %41.png")
    );
    let got = b.get(&p, None).await.expect("get");
    assert_eq!(got.range, 0..10);
    assert_eq!(got.head.content_type.as_deref(), Some("application/x-test"));
    assert_eq!(got.bytes().await.unwrap(), b"0123456789");

    // Ranges.
    let got = b
        .get(&p, Some(ByteRange::new(2, 5).unwrap()))
        .await
        .expect("range");
    assert_eq!(got.range, 2..5);
    assert_eq!(got.bytes().await.unwrap(), b"234");
    let got = b
        .get(&p, Some(ByteRange::from(7)))
        .await
        .expect("open range");
    assert_eq!(got.bytes().await.unwrap(), b"789");
    let got = b
        .get(&p, Some(ByteRange::new(8, 100).unwrap()))
        .await
        .expect("cut range");
    assert_eq!(got.bytes().await.unwrap(), b"89");
    assert!(matches!(
        b.get(&p, Some(ByteRange::from(10))).await,
        Err(StorageError::InvalidRange)
    ));

    // A stream, in chunks.
    let s = path(base, "one/streamed");
    let head = b
        .put(
            &s,
            chunks(vec![b"abc".to_vec(), b"".to_vec(), b"defg".to_vec()]),
            PutOptions::new(),
        )
        .await
        .expect("streamed put");
    assert_eq!(head.size, 7);
    assert_eq!(
        b.get(&s, None).await.unwrap().bytes().await.unwrap(),
        b"abcdefg"
    );

    // The limit, for bytes and for a stream: nothing is stored.
    let l = path(base, "one/limited");
    assert!(matches!(
        b.put(
            &l,
            vec![0u8; 11].into(),
            PutOptions::new().with_max_bytes(10)
        )
        .await,
        Err(StorageError::TooLarge {
            size: 11,
            limit: 10
        })
    ));
    assert!(matches!(
        b.put(
            &l,
            chunks(vec![vec![0u8; 6], vec![0u8; 6]]),
            PutOptions::new().with_max_bytes(10)
        )
        .await,
        Err(StorageError::TooLarge { limit: 10, .. })
    ));
    assert!(matches!(b.head(&l).await, Err(StorageError::NotFound)));

    // Create only refuses a taken path and keeps what is there; a plain
    // put replaces.
    if can.create_only {
        assert!(matches!(
            b.put(
                &p,
                b"other".as_slice().into(),
                PutOptions::new().create_only()
            )
            .await,
            Err(StorageError::Exists)
        ));
        assert_eq!(
            b.get(&p, None).await.unwrap().bytes().await.unwrap(),
            b"0123456789"
        );
    }
    b.put(&p, b"replaced".as_slice().into(), PutOptions::new())
        .await
        .expect("overwrite");
    assert_eq!(
        b.get(&p, None).await.unwrap().bytes().await.unwrap(),
        b"replaced"
    );
    let fresh = path(base, "one/fresh");
    b.put(
        &fresh,
        b"new".as_slice().into(),
        PutOptions::new().create_only(),
    )
    .await
    .expect("create only on a free path");

    // A body large enough to go up in parts.
    if can.big {
        let big = path(base, "big");
        let part = vec![7u8; 3 * 1024 * 1024];
        let head = b
            .put(
                &big,
                chunks(vec![part.clone(), part.clone(), part.clone(), part]),
                PutOptions::new().with_content_type("application/octet-stream"),
            )
            .await
            .expect("multipart put");
        assert_eq!(head.size, 12 * 1024 * 1024);
        assert_eq!(b.head(&big).await.unwrap().size, 12 * 1024 * 1024);
        let tail = b
            .get(&big, Some(ByteRange::from(12 * 1024 * 1024 - 3)))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(tail, [7, 7, 7]);
        b.delete(&big).await.unwrap();
    }

    // List: by directory, by string prefix, and the whole run.
    for name in ["l/a/1", "l/a/2", "l/a/b/3", "l/ab/4"] {
        b.put(&path(base, name), name.as_bytes().into(), PutOptions::new())
            .await
            .expect("put for list");
    }
    let names = |list: Vec<crate::bucket::Listed>| {
        let mut out: Vec<String> = list
            .into_iter()
            .map(|l| l.path.as_str()[base.len() + 1..].to_string())
            .collect();
        out.sort();
        out
    };
    assert_eq!(
        names(b.list(&format!("{base}/l/a/")).await.unwrap()),
        ["l/a/1", "l/a/2", "l/a/b/3"]
    );
    assert_eq!(
        names(b.list(&format!("{base}/l/a")).await.unwrap()),
        ["l/a/1", "l/a/2", "l/a/b/3", "l/ab/4"]
    );
    let all = b.list(&format!("{base}/")).await.unwrap();
    assert!(all.iter().any(|l| l.path == p && l.size == 8), "{all:?}");
    assert!(b.list("../").await.is_err());

    // Delete is idempotent.
    b.delete(&path(base, "l/a/1")).await.unwrap();
    b.delete(&path(base, "l/a/1")).await.unwrap();
    assert!(matches!(
        b.head(&path(base, "l/a/1")).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        b.get(&path(base, "l/a/1"), None).await,
        Err(StorageError::NotFound)
    ));
    assert_eq!(b.list(&format!("{base}/l/a/")).await.unwrap().len(), 2);

    // Signed URLs, both ways.
    assert!(!can.signs || b.can_sign());
    let ttl = Duration::from_secs(120);
    let none = SignedGet::none();
    let get = b
        .signed_url(SignedMethod::Get, &p, ttl, &none)
        .await
        .expect("sign");
    let put_path = path(base, "signed/put");
    let put = b
        .signed_url(SignedMethod::Put, &put_path, ttl, &none)
        .await
        .expect("sign");
    // A GET with the download rule's overrides, inside the signature.
    let overrides = SignedGet::for_object(&ContentType::octet_stream(), None);
    let with = b
        .signed_url(SignedMethod::Get, &p, ttl, &overrides)
        .await
        .expect("sign with overrides");
    if can.signs {
        let (get, put) = (get.expect("signs a get"), put.expect("signs a put"));
        let with = with.expect("signs a get with overrides");
        assert_eq!(
            query_value(&with, "response-content-type").as_deref(),
            Some("application/octet-stream")
        );
        assert_eq!(
            query_value(&with, "response-content-disposition").as_deref(),
            Some("attachment; filename=\"download\"")
        );
        // The signature covers them: S3 and GCS list the query in the
        // canonical request, and name the signed headers in the URL.
        assert!(
            query_value(&with, "X-Amz-SignedHeaders").is_some()
                || query_value(&with, "X-Goog-SignedHeaders").is_some(),
            "{with}"
        );
        assert!(
            b.signed_url(SignedMethod::Put, &put_path, ttl, &overrides)
                .await
                .is_err(),
            "a PUT takes no overrides"
        );
        if let Some(fetch) = can.fetch {
            assert_eq!(fetch.get(&get).await.expect("signed get"), b"replaced");
            fetch
                .put(&put, b"through a signed url".to_vec())
                .await
                .expect("signed put");
            assert_eq!(
                b.get(&put_path, None).await.unwrap().bytes().await.unwrap(),
                b"through a signed url"
            );
            let got = fetch.send("GET", &with).await.expect("signed get");
            assert_eq!(got.status, 200, "{got:?}");
            assert_eq!(got.body, b"replaced");
            if can.verifies {
                // Both overrides come back as response headers.
                assert_eq!(got.header("content-type"), Some("application/octet-stream"));
                assert_eq!(
                    got.header("content-disposition"),
                    Some("attachment; filename=\"download\"")
                );
                // The bearer cannot drop the attachment: the signature then
                // fails.
                let tampered = without_param(&with, "response-content-disposition");
                assert_ne!(tampered, with);
                let got = fetch.send("GET", &tampered).await.expect("tampered");
                assert_eq!(got.status, 403, "a tampered URL must be refused");
                // A URL signed for GET refuses HEAD: the reason the route
                // answers HEAD itself (D30).
                let got = fetch.send("HEAD", &with).await.expect("head");
                assert_eq!(got.status, 403, "a GET URL must refuse HEAD");
                // An expired URL is refused.
                let short = b
                    .signed_url(SignedMethod::Get, &p, Duration::from_secs(1), &overrides)
                    .await
                    .expect("sign")
                    .expect("a url");
                std::thread::sleep(Duration::from_secs(3));
                let got = fetch.send("GET", &short).await.expect("expired");
                assert_eq!(got.status, 403, "an expired URL must be refused");
            } else {
                // The emulator may not echo the overrides; when it does,
                // they are right.
                if let Some(ct) = got.header("content-type") {
                    if got.header("content-disposition").is_some() {
                        assert_eq!(ct, "application/octet-stream");
                    }
                }
                if let Some(cd) = got.header("content-disposition") {
                    assert_eq!(cd, "attachment; filename=\"download\"");
                }
            }
        }
    } else {
        assert_eq!((get, put, with), (None, None, None));
    }

    // A stored disposition comes back on a cloud head; a disk keeps none.
    let d = path(base, "one/disposed");
    b.put(
        &d,
        b"x".as_slice().into(),
        PutOptions::new().with_content_disposition("attachment; filename=\"d.bin\""),
    )
    .await
    .expect("put with a disposition");
    let stored = b.head(&d).await.expect("head").content_disposition;
    if can.disposition {
        assert_eq!(stored.as_deref(), Some("attachment; filename=\"d.bin\""));
    } else if !can.signs {
        assert_eq!(stored, None, "a disk keeps no disposition");
    }

    // Clean the run's prefix.
    for listed in b.list(&format!("{base}/")).await.unwrap() {
        b.delete(&listed.path).await.unwrap();
    }
    assert!(b.list(&format!("{base}/")).await.unwrap().is_empty());
}
