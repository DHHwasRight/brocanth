//! The tests, and the first two modules are the ones that matter: an address
//! that cannot spell a path, and a store that proves it on a real disk.

use std::path::{Path, PathBuf};

use crate::key::{HashKey, ObjectKey};
use crate::local::LocalDisk;
use crate::meta::{ContentType, Filename, ObjectInfo};
use crate::rt::block_on;
use crate::spec::{CropAnchor, VariantMode, VariantSpec};
use crate::storage::Storage;
use crate::token::{FileTokenKey, FileTokens};
use crate::StorageError;

/// Every way a person has ever tried to leave a directory.
const TRAVERSALS: &[&str] = &[
    "../../etc/passwd",
    "../../../../../../../../etc/passwd",
    "..",
    ".",
    "./",
    "../",
    "..\\..\\windows\\system32\\config\\sam",
    "....//....//etc/passwd",
    "..;/etc/passwd",
    "%2e%2e%2f%2e%2e%2fetc%2fpasswd",
    "%2E%2E/%2E%2E/etc/passwd",
    "..%252f..%252fetc",
    "%c0%ae%c0%ae/etc/passwd",
    "..%c0%af..%c0%afetc%c0%afpasswd",
    "/etc/passwd",
    "/",
    "//etc/passwd",
    "\\\\server\\share\\secret",
    "C:\\Windows\\win.ini",
    "c:/windows/win.ini",
    "file:///etc/passwd",
    "\u{ff0e}\u{ff0e}/etc/passwd",
    "\u{2215}etc\u{2215}passwd",
    "etc/passwd\u{0}.png",
    "\0",
    "\n../etc/passwd",
    "a/../../b",
    "~/.ssh/id_rsa",
    "$HOME/.ssh/id_rsa",
];

mod addresses {
    use super::*;

    #[test]
    fn no_traversal_parses_as_an_address() {
        for attempt in TRAVERSALS {
            assert!(
                matches!(ObjectKey::parse(attempt), Err(StorageError::InvalidKey)),
                "{attempt:?} parsed as an address"
            );
        }
    }

    #[test]
    fn only_sixty_four_lowercase_hex_parses() {
        let valid = "0".repeat(64);
        assert!(ObjectKey::parse(&valid).is_ok());
        for bad in [
            "",
            "0",
            &"0".repeat(63),
            &"0".repeat(65),
            &"z".repeat(64),
            &"A".repeat(64),
            &format!("{}/", "0".repeat(63)),
            &format!("{}.", "0".repeat(63)),
            &format!(" {}", "0".repeat(64)),
        ] {
            assert!(
                ObjectKey::parse(bad).is_err(),
                "{bad:?} parsed as an address"
            );
        }
    }

    #[test]
    fn the_hex_form_is_always_sixty_four_safe_characters() {
        // The structural claim, stated as a property: whatever the bytes, the
        // rendering is drawn from one closed, path-safe alphabet.
        for seed in 0u8..=255 {
            let key = ObjectKey::of(&[seed; 7]);
            let hex = key.to_hex();
            assert_eq!(hex.len(), 64);
            assert!(hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            assert_eq!(ObjectKey::parse(&hex).unwrap(), key);
        }
    }

    #[test]
    fn content_addressing_dedupes_and_verifies() {
        let a = ObjectKey::of(b"hello");
        let b = ObjectKey::of(b"hello");
        assert_eq!(a, b);
        assert_ne!(a, ObjectKey::of(b"hellp"));
        assert!(a.verifies(b"hello"));
        assert!(!a.verifies(b"hellp"));
    }

    #[test]
    fn a_keyed_address_still_dedupes_but_cannot_be_guessed() {
        let key = HashKey::from_secret(b"deployment secret");
        let other = HashKey::from_secret(b"another deployment");
        assert_eq!(
            ObjectKey::keyed(&key, b"hello"),
            ObjectKey::keyed(&key, b"hello")
        );
        // Without the key, the file in hand does not give you the address.
        assert_ne!(ObjectKey::keyed(&key, b"hello"), ObjectKey::of(b"hello"));
        assert_ne!(
            ObjectKey::keyed(&key, b"hello"),
            ObjectKey::keyed(&other, b"hello")
        );
        // And the integrity check is for plain addressing only, which is what
        // its documentation says.
        assert!(!ObjectKey::keyed(&key, b"hello").verifies(b"hello"));
    }

    #[test]
    fn a_random_address_does_not_dedupe() {
        let a = ObjectKey::random().unwrap();
        let b = ObjectKey::random().unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn the_address_is_an_etag() {
        let key = ObjectKey::of(b"hello");
        assert_eq!(key.etag(), format!("\"{}\"", key.to_hex()));
    }

    #[test]
    fn a_secret_is_not_printed() {
        assert_eq!(
            format!("{:?}", HashKey::from_secret(b"s")),
            "HashKey(<redacted>)"
        );
        assert_eq!(
            format!("{:?}", FileTokenKey::from_secret(b"s")),
            "FileTokenKey(<redacted>)"
        );
    }
}

mod filenames {
    use super::*;

    #[test]
    fn a_traversal_cannot_survive_sanitising() {
        for attempt in TRAVERSALS {
            if let Some(name) = Filename::sanitise(attempt) {
                let text = name.as_str();
                assert!(!text.contains('/'), "{attempt:?} kept a separator");
                assert!(!text.contains('\\'), "{attempt:?} kept a separator");
                assert!(!text.starts_with('.'), "{attempt:?} kept a leading dot");
                assert!(!text.contains('\0'), "{attempt:?} kept a NUL");
                assert!(!text.chars().any(char::is_control));
                assert!(text.len() <= crate::MAX_FILENAME_LEN);
            }
        }
        assert_eq!(
            Filename::sanitise("../../etc/passwd").unwrap().as_str(),
            "etcpasswd"
        );
        assert_eq!(
            Filename::sanitise("..").map(|n| n.as_str().to_string()),
            None
        );
        assert_eq!(
            Filename::sanitise("/").map(|n| n.as_str().to_string()),
            None
        );
    }

    #[test]
    fn a_right_to_left_override_is_removed() {
        // `exe\u{202e}gnp.txt` renders as `exetxt.png` in a file listing.
        let name = Filename::sanitise("invoice\u{202e}gnp.exe").unwrap();
        assert_eq!(name.as_str(), "invoicegnp.exe");
    }

    #[test]
    fn a_long_name_is_truncated_rather_than_refused() {
        let name = Filename::sanitise(&"a".repeat(10_000)).unwrap();
        assert_eq!(name.as_str().len(), crate::MAX_FILENAME_LEN);
    }

    #[test]
    fn an_extension_is_lowercased() {
        assert_eq!(
            Filename::sanitise("Photo.JPEG").unwrap().extension(),
            Some("jpeg".to_string())
        );
        assert_eq!(Filename::sanitise("plain").unwrap().extension(), None);
    }
}

mod content_types {
    use super::*;

    #[test]
    fn a_header_injection_is_not_a_media_type() {
        for attempt in [
            "text/html\r\nX-Evil: 1",
            "text/html\nSet-Cookie: a=b",
            "text/html\0",
            "text/ html",
            "texthtml",
            "/html",
            "text/",
            "",
            "   ",
            "text/html; charset=\"utf-8\r\n\"",
        ] {
            assert!(
                ContentType::parse(attempt).is_none(),
                "{attempt:?} parsed as a media type"
            );
        }
    }

    #[test]
    fn the_essence_is_lowercased_and_parameters_are_kept() {
        let ct = ContentType::parse("TEXT/Plain; Charset=UTF-8").unwrap();
        assert_eq!(ct.as_str(), "text/plain; charset=UTF-8");
        assert_eq!(ct.essence(), "text/plain");
        assert!(!ct.is_safe_to_inline());
    }

    #[test]
    fn sniffing_knows_four_formats_and_no_more() {
        assert_eq!(
            ContentType::sniff(b"\x89PNG\r\n\x1a\n...")
                .unwrap()
                .as_str(),
            "image/png"
        );
        assert_eq!(
            ContentType::sniff(b"\xff\xd8\xff\xe0").unwrap().as_str(),
            "image/jpeg"
        );
        assert_eq!(
            ContentType::sniff(b"GIF89a....").unwrap().as_str(),
            "image/gif"
        );
        assert_eq!(
            ContentType::sniff(b"RIFF\0\0\0\0WEBPVP8 ")
                .unwrap()
                .as_str(),
            "image/webp"
        );
        assert!(ContentType::sniff(b"<!DOCTYPE html>").is_none());
        assert!(ContentType::sniff(b"").is_none());
        assert!(ContentType::sniff(b"\x89PN").is_none());
    }

    #[test]
    fn only_images_are_inline_safe() {
        assert!(ContentType::parse("image/png").unwrap().is_safe_to_inline());
        assert!(!ContentType::parse("text/html").unwrap().is_safe_to_inline());
        assert!(!ContentType::parse("image/svg+xml")
            .unwrap()
            .is_safe_to_inline());
        assert!(!ContentType::octet_stream().is_safe_to_inline());
    }
}

mod specs {
    use super::*;

    #[test]
    fn the_whole_syntax_parses() {
        let cases = [
            ("100x200", 100, 200, VariantMode::Crop(CropAnchor::Centre)),
            ("100x200t", 100, 200, VariantMode::Crop(CropAnchor::Top)),
            ("100x200b", 100, 200, VariantMode::Crop(CropAnchor::Bottom)),
            ("100x200f", 100, 200, VariantMode::Fit),
            ("0x200", 0, 200, VariantMode::Scale),
            ("100x0", 100, 0, VariantMode::Scale),
            ("1x1", 1, 1, VariantMode::Crop(CropAnchor::Centre)),
        ];
        for (text, width, height, mode) in cases {
            let spec = VariantSpec::parse(text).unwrap();
            assert_eq!(
                (spec.width(), spec.height(), spec.mode()),
                (width, height, mode)
            );
            // The canonical spelling is what the cache path uses, so it has to
            // round-trip exactly.
            assert_eq!(spec.to_string(), text);
        }
    }

    #[test]
    fn nothing_else_parses() {
        for attempt in [
            "",
            "x",
            "100",
            "100x",
            "x100",
            "0x0",
            "100x200z",
            "100x200T",
            "100x200ff",
            "100x200 ",
            " 100x200",
            "+100x200",
            "-100x200",
            "100X200",
            "100x200x300",
            "007x200",
            "00x200",
            "0100x200",
            "100x0t",
            "0x200f",
            "99999x1",
            "5000x5000",
            "1e3x200",
            "100x200%2f..",
            "../../etc/passwd",
            "100x200/../../etc",
            "100x200\0",
            "100x200\n",
        ] {
            assert!(
                VariantSpec::parse(attempt).is_err(),
                "{attempt:?} parsed as a size"
            );
        }
        for attempt in TRAVERSALS {
            assert!(
                VariantSpec::parse(attempt).is_err(),
                "{attempt:?} parsed as a size"
            );
        }
    }

    #[test]
    fn the_constructor_checks_what_the_parser_checks() {
        assert!(VariantSpec::new(10, 10, VariantMode::Fit).is_ok());
        assert!(VariantSpec::new(0, 10, VariantMode::Scale).is_ok());
        assert!(VariantSpec::new(0, 0, VariantMode::Scale).is_err());
        assert!(VariantSpec::new(10, 10, VariantMode::Scale).is_err());
        assert!(VariantSpec::new(0, 10, VariantMode::Fit).is_err());
        assert!(VariantSpec::new(crate::MAX_VARIANT_DIM + 1, 10, VariantMode::Fit).is_err());
    }

    #[test]
    fn a_rendered_spec_is_always_one_safe_path_component() {
        for width in [0u32, 1, 7, 4096] {
            for height in [0u32, 1, 7, 4096] {
                for mode in [
                    VariantMode::Crop(CropAnchor::Centre),
                    VariantMode::Crop(CropAnchor::Top),
                    VariantMode::Crop(CropAnchor::Bottom),
                    VariantMode::Fit,
                    VariantMode::Scale,
                ] {
                    if let Ok(spec) = VariantSpec::new(width, height, mode) {
                        let text = spec.to_string();
                        assert!(text
                            .bytes()
                            .all(|b| b.is_ascii_digit() || matches!(b, b'x' | b't' | b'b' | b'f')));
                        assert_eq!(VariantSpec::parse(&text).unwrap(), spec);
                    }
                }
            }
        }
    }
}

mod disk {
    use super::*;

    fn store() -> (tempfile::TempDir, LocalDisk) {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let store = LocalDisk::open(dir.path()).expect("a store");
        (dir, store)
    }

    fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("readable") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                walk(&path, into);
            } else {
                into.push(path);
            }
        }
    }

    #[test]
    fn an_object_round_trips() {
        let (_dir, store) = store();
        let about = ObjectInfo::new()
            .with_filename("holiday.png")
            .with_declared_type("image/png");
        let key = store
            .put_blocking(b"\x89PNG\r\n\x1a\nbody", &about)
            .unwrap();

        assert!(store.exists_blocking(&key).unwrap());
        assert_eq!(store.get_blocking(&key).unwrap(), b"\x89PNG\r\n\x1a\nbody");

        let meta = store.meta_blocking(&key).unwrap();
        assert_eq!(meta.key, key);
        assert_eq!(meta.size, 12);
        assert_eq!(meta.content_type.as_str(), "image/png");
        assert_eq!(meta.filename.unwrap().as_str(), "holiday.png");

        store.delete_blocking(&key).unwrap();
        assert!(!store.exists_blocking(&key).unwrap());
        assert!(matches!(
            store.get_blocking(&key),
            Err(StorageError::NotFound)
        ));
        assert!(matches!(
            store.meta_blocking(&key),
            Err(StorageError::NotFound)
        ));
        // Idempotent, because a retried delete must not fail.
        store.delete_blocking(&key).unwrap();
    }

    #[test]
    fn a_user_supplied_name_never_leaves_the_root() {
        let (dir, store) = store();
        let root = store.root().to_path_buf();
        for attempt in TRAVERSALS {
            let about = ObjectInfo::new()
                .with_filename(attempt)
                .with_declared_type("text/html");
            let key = store
                .put_blocking(format!("payload for {attempt}").as_bytes(), &about)
                .unwrap();
            let spec = VariantSpec::parse("10x10").unwrap();
            store.put_variant_blocking(&key, spec, b"derived").unwrap();
        }

        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(!files.is_empty());
        for path in &files {
            assert!(path.starts_with(&root), "{path:?} escaped the root");
            // Every component below the root is hex, a fixed directory name, a
            // metadata sidecar or a canonical size. Nothing a client sent.
            for component in path.strip_prefix(&root).unwrap().components() {
                let text = component.as_os_str().to_string_lossy().to_string();
                let text = text.strip_suffix(".meta").unwrap_or(&text).to_string();
                let allowed = matches!(text.as_str(), "objects" | "variants" | "tmp")
                    || text.bytes().all(|b| b.is_ascii_hexdigit())
                    || VariantSpec::parse(&text).is_ok();
                assert!(allowed, "{text:?} is not a name this crate generates");
            }
        }
        // And the obvious end of it: nothing called `passwd`, anywhere.
        assert!(!files
            .iter()
            .any(|p| p.file_name().is_some_and(|n| n == "passwd")));
        // Nor did anything appear beside the root.
        let mut siblings = Vec::new();
        walk(dir.path(), &mut siblings);
        assert_eq!(siblings.len(), files.len());
    }

    #[test]
    fn identical_content_is_stored_once_and_keeps_the_first_metadata() {
        let (_dir, store) = store();
        let first = ObjectInfo::new()
            .with_filename("mine.png")
            .with_declared_type("image/png");
        let second = ObjectInfo::new()
            .with_filename("yours.html")
            .with_declared_type("text/html");

        let a = store.put_blocking(b"shared bytes", &first).unwrap();
        let b = store.put_blocking(b"shared bytes", &second).unwrap();
        assert_eq!(a, b);

        // The second uploader must not be able to change what the first
        // person's file is served as.
        let meta = store.meta_blocking(&a).unwrap();
        assert_eq!(meta.filename.unwrap().as_str(), "mine.png");
        assert_eq!(meta.content_type.as_str(), "image/png");

        let mut files = Vec::new();
        walk(&store.root().join("objects"), &mut files);
        assert_eq!(files.len(), 2, "one object and one sidecar");
    }

    #[test]
    fn the_content_decides_the_media_type() {
        let (_dir, store) = store();
        // A PNG labelled as HTML is served as a PNG.
        let lying = ObjectInfo::new().with_declared_type("text/html");
        let key = store.put_blocking(b"\x89PNG\r\n\x1a\nx", &lying).unwrap();
        assert_eq!(
            store.meta_blocking(&key).unwrap().content_type.as_str(),
            "image/png"
        );

        // Something unrecognised keeps the declared type, and the serving half
        // is told not to render it inline.
        let declared = ObjectInfo::new().with_declared_type("application/pdf");
        let key = store.put_blocking(b"%PDF-1.7", &declared).unwrap();
        let meta = store.meta_blocking(&key).unwrap();
        assert_eq!(meta.content_type.as_str(), "application/pdf");
        assert!(!meta.content_type.is_safe_to_inline());

        // Nothing claimed and nothing recognised.
        let key = store.put_blocking(b"anything", &ObjectInfo::new()).unwrap();
        assert_eq!(
            store.meta_blocking(&key).unwrap().content_type.as_str(),
            "application/octet-stream"
        );
    }

    #[test]
    fn a_keyed_store_addresses_differently_but_behaves_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalDisk::open(dir.path())
            .unwrap()
            .with_hash_key(HashKey::from_secret(b"deployment secret"));
        let key = store.put_blocking(b"hello", &ObjectInfo::new()).unwrap();
        assert_ne!(key, ObjectKey::of(b"hello"));
        assert_eq!(store.address_of(b"hello"), key);
        assert_eq!(store.get_blocking(&key).unwrap(), b"hello");
        // The plain address of the same bytes is not stored, which is the
        // whole point: holding the file does not tell you where it lives.
        assert!(!store.exists_blocking(&ObjectKey::of(b"hello")).unwrap());
    }

    #[test]
    fn a_caller_chosen_address_does_not_dedupe() {
        let (_dir, store) = store();
        let a = ObjectKey::random().unwrap();
        let b = ObjectKey::random().unwrap();
        store
            .put_at_blocking(&a, b"same", &ObjectInfo::new())
            .unwrap();
        store
            .put_at_blocking(&b, b"same", &ObjectInfo::new())
            .unwrap();
        assert_eq!(store.get_blocking(&a).unwrap(), b"same");
        assert_eq!(store.get_blocking(&b).unwrap(), b"same");
        store.delete_blocking(&a).unwrap();
        // Deleting one leaves the other, which is what a random address buys.
        assert!(store.exists_blocking(&b).unwrap());
    }

    #[test]
    fn an_existing_address_is_never_overwritten() {
        let (_dir, store) = store();
        let key = ObjectKey::random().unwrap();
        store
            .put_at_blocking(&key, b"first", &ObjectInfo::new())
            .unwrap();
        store
            .put_at_blocking(&key, b"second", &ObjectInfo::new())
            .unwrap();
        assert_eq!(store.get_blocking(&key).unwrap(), b"first");
    }

    #[test]
    fn deleting_an_object_deletes_its_variants() {
        let (_dir, store) = store();
        let key = store.put_blocking(b"source", &ObjectInfo::new()).unwrap();
        let spec = VariantSpec::parse("50x50t").unwrap();
        store.put_variant_blocking(&key, spec, b"thumb").unwrap();
        assert_eq!(
            store.get_variant_blocking(&key, spec).unwrap(),
            Some(b"thumb".to_vec())
        );

        store.delete_blocking(&key).unwrap();
        // A thumbnail of a deleted document is still the document.
        assert_eq!(store.get_variant_blocking(&key, spec).unwrap(), None);
    }

    #[test]
    fn a_missing_variant_is_not_an_error() {
        let (_dir, store) = store();
        let key = store.put_blocking(b"source", &ObjectInfo::new()).unwrap();
        let spec = VariantSpec::parse("10x10f").unwrap();
        assert_eq!(store.get_variant_blocking(&key, spec).unwrap(), None);
    }

    #[test]
    fn nothing_is_left_in_the_temporary_directory() {
        let (_dir, store) = store();
        for i in 0..20 {
            store
                .put_blocking(format!("body {i}").as_bytes(), &ObjectInfo::new())
                .unwrap();
        }
        let mut leftovers = Vec::new();
        walk(&store.root().join("tmp"), &mut leftovers);
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn the_store_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, store) = store();
        let key = store.put_blocking(b"private", &ObjectInfo::new()).unwrap();
        let mut files = Vec::new();
        walk(store.root(), &mut files);
        assert!(!files.is_empty());
        for path in files {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "{path:?} is readable by others");
        }
        let mode = std::fs::metadata(store.root().join("objects"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
        let _ = key;
    }

    #[test]
    fn the_trait_works_through_a_trait_object() {
        let dir = tempfile::tempdir().unwrap();
        let store: std::sync::Arc<dyn Storage> =
            std::sync::Arc::new(LocalDisk::open(dir.path()).unwrap());
        block_on(async {
            let key = store
                .put(b"\x89PNG\r\n\x1a\nx", &ObjectInfo::new())
                .await
                .unwrap();
            assert!(store.exists(&key).await.unwrap());
            assert_eq!(store.get(&key).await.unwrap(), b"\x89PNG\r\n\x1a\nx");
            assert_eq!(store.meta(&key).await.unwrap().size, 9);
            let spec = VariantSpec::parse("8x8").unwrap();
            assert_eq!(store.get_variant(&key, spec).await.unwrap(), None);
            store.put_variant(&key, spec, b"thumb").await.unwrap();
            assert_eq!(
                store.get_variant(&key, spec).await.unwrap(),
                Some(b"thumb".to_vec())
            );
            store.delete(&key).await.unwrap();
            assert!(!store.exists(&key).await.unwrap());
        });
    }

    #[test]
    fn concurrent_puts_of_one_file_agree() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(LocalDisk::open(dir.path()).unwrap());
        let keys: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let store = std::sync::Arc::clone(&store);
                    scope.spawn(move || {
                        store
                            .put_blocking(b"the same bytes", &ObjectInfo::new())
                            .unwrap()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(keys.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(store.get_blocking(&keys[0]).unwrap(), b"the same bytes");
    }
}

mod tokens {
    use super::*;

    fn tokens() -> FileTokens {
        FileTokens::new(FileTokenKey::from_secret(b"deployment secret"))
    }

    #[test]
    fn a_token_opens_the_object_it_was_minted_for() {
        let tokens = tokens();
        let key = ObjectKey::of(b"private");
        let token = tokens.issue_at(&key, 1_700_000_000);
        assert_eq!(token.key, key);
        assert_eq!(token.expires_at, 1_700_000_000 + crate::DEFAULT_TTL);
        assert_eq!(
            tokens.verify_at(token.as_str(), 1_700_000_000).unwrap(),
            key
        );
    }

    #[test]
    fn a_token_expires() {
        let tokens = tokens().with_ttl(60);
        let key = ObjectKey::of(b"private");
        let token = tokens.issue_at(&key, 1_000);
        assert!(tokens.verify_at(token.as_str(), 1_059).is_ok());
        // On the second it expires, not after it.
        assert!(matches!(
            tokens.verify_at(token.as_str(), 1_060),
            Err(StorageError::InvalidToken)
        ));
        assert!(matches!(
            tokens.verify_at(token.as_str(), 9_999),
            Err(StorageError::InvalidToken)
        ));
    }

    #[test]
    fn the_expiry_cannot_be_extended() {
        let tokens = tokens().with_ttl(60);
        let key = ObjectKey::of(b"private");
        let token = tokens.issue_at(&key, 1_000).into_string();
        let (payload, mac) = token.split_once('.').unwrap();
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        let mut bytes = URL_SAFE_NO_PAD.decode(payload).unwrap();
        let len = bytes.len();
        bytes[len - 1] = 0xff;
        let forged = format!("{}.{}", URL_SAFE_NO_PAD.encode(&bytes), mac);
        assert!(matches!(
            tokens.verify_at(&forged, 1_000),
            Err(StorageError::InvalidToken)
        ));
    }

    #[test]
    fn a_token_cannot_be_pointed_at_another_object() {
        let tokens = tokens();
        let mine = ObjectKey::of(b"mine");
        let yours = ObjectKey::of(b"yours");
        let token = tokens.issue_at(&mine, 0).into_string();
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        let (payload, mac) = token.split_once('.').unwrap();
        let mut bytes = URL_SAFE_NO_PAD.decode(payload).unwrap();
        bytes[1..1 + crate::KEY_BYTES].copy_from_slice(yours.as_bytes());
        let forged = format!("{}.{}", URL_SAFE_NO_PAD.encode(&bytes), mac);
        assert!(matches!(
            tokens.verify_at(&forged, 0),
            Err(StorageError::InvalidToken)
        ));
        // And the honest one still only opens what it names.
        assert_eq!(tokens.verify_at(&token, 0).unwrap(), mine);
    }

    #[test]
    fn another_key_does_not_verify() {
        let key = ObjectKey::of(b"private");
        let token = tokens().issue_at(&key, 0).into_string();
        let other = FileTokens::new(FileTokenKey::from_secret(b"another deployment"));
        assert!(matches!(
            other.verify_at(&token, 0),
            Err(StorageError::InvalidToken)
        ));
    }

    #[test]
    fn the_two_key_derivations_are_separated() {
        // One deployment secret feeds both an address key and a token key.
        // Signing a token with the address key's material must produce
        // something the token key refuses, or the two uses share a signature.
        use sha2::{Digest, Sha256};
        let secret = b"deployment secret";
        let mut hash = Sha256::new();
        hash.update(b"brocanth-storage/address-key/v1");
        hash.update(secret);
        let address_material: [u8; 32] = hash.finalize().into();

        let key = ObjectKey::of(b"x");
        let token = FileTokens::new(FileTokenKey::from_bytes(address_material))
            .issue_at(&key, 0)
            .into_string();
        let honest = FileTokens::new(FileTokenKey::from_secret(secret));
        assert!(matches!(
            honest.verify_at(&token, 0),
            Err(StorageError::InvalidToken)
        ));
    }

    #[test]
    fn rubbish_is_rejected_without_an_explanation() {
        let tokens = tokens();
        for attempt in [
            "",
            ".",
            "..",
            "a.b",
            "....",
            "not base64!.also not",
            "AAAA.AAAA",
            &"A".repeat(300),
            "../../etc/passwd",
            &format!("{}.{}", "A".repeat(55), "A".repeat(43)),
        ] {
            assert!(
                matches!(
                    tokens.verify_at(attempt, 0),
                    Err(StorageError::InvalidToken)
                ),
                "{attempt:?} verified"
            );
        }
    }

    #[test]
    fn a_token_is_not_printed() {
        let token = tokens().issue_at(&ObjectKey::of(b"x"), 0);
        let shown = format!("{token:?}");
        assert!(shown.contains("<redacted>"));
        assert!(!shown.contains(token.as_str()));
    }

    #[test]
    fn a_generated_key_works_and_differs() {
        let key = ObjectKey::of(b"x");
        let a = FileTokens::new(FileTokenKey::generate().unwrap());
        let b = FileTokens::new(FileTokenKey::generate().unwrap());
        let token = a.issue_at(&key, 0).into_string();
        assert_eq!(a.verify_at(&token, 0).unwrap(), key);
        assert!(b.verify_at(&token, 0).is_err());
    }

    #[test]
    fn the_system_clock_path_works_too() {
        let tokens = tokens();
        let key = ObjectKey::of(b"x");
        let token = tokens.issue(&key);
        assert_eq!(tokens.verify(token.as_str()).unwrap(), key);
        assert!(crate::token::now() > 1_700_000_000);
    }
}

#[cfg(feature = "variants")]
mod images {
    use super::*;
    use crate::variants::{derive_variant, render};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut image = image::RgbImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            // A gradient, so a crop anchored at the top and one anchored at
            // the bottom are distinguishable.
            *pixel = image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]);
        }
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn dimensions(bytes: &[u8]) -> (u32, u32) {
        image::load_from_memory(bytes)
            .map(|i| (i.width(), i.height()))
            .unwrap()
    }

    #[test]
    fn a_crop_is_exactly_the_requested_size() {
        let source = png(400, 200);
        for spec in ["100x100", "100x100t", "100x100b"] {
            let variant = render(&source, VariantSpec::parse(spec).unwrap()).unwrap();
            assert_eq!(dimensions(&variant.bytes), (100, 100), "{spec}");
            assert_eq!(variant.content_type.as_str(), "image/jpeg");
            assert!(variant.content_type.is_safe_to_inline());
        }
    }

    #[test]
    fn the_anchor_changes_which_pixels_survive() {
        let source = png(200, 400);
        let top = render(&source, VariantSpec::parse("100x100t").unwrap()).unwrap();
        let bottom = render(&source, VariantSpec::parse("100x100b").unwrap()).unwrap();
        let centre = render(&source, VariantSpec::parse("100x100").unwrap()).unwrap();
        assert_ne!(top.bytes, bottom.bytes);
        assert_ne!(top.bytes, centre.bytes);
        assert_ne!(centre.bytes, bottom.bytes);
    }

    #[test]
    fn a_fit_never_crops() {
        let source = png(400, 200);
        let variant = render(&source, VariantSpec::parse("100x100f").unwrap()).unwrap();
        // 400x200 inside 100x100 is 100x50: the aspect ratio is preserved and
        // one axis comes out short.
        assert_eq!(dimensions(&variant.bytes), (100, 50));
    }

    #[test]
    fn a_free_dimension_follows_the_aspect_ratio() {
        let source = png(400, 200);
        let by_width = render(&source, VariantSpec::parse("100x0").unwrap()).unwrap();
        assert_eq!(dimensions(&by_width.bytes), (100, 50));
        let by_height = render(&source, VariantSpec::parse("0x100").unwrap()).unwrap();
        assert_eq!(dimensions(&by_height.bytes), (200, 100));
    }

    #[test]
    fn transparency_survives_as_a_png() {
        let mut image = image::RgbaImage::new(40, 40);
        for pixel in image.pixels_mut() {
            *pixel = image::Rgba([10, 20, 30, 0]);
        }
        let mut source = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut source),
                image::ImageFormat::Png,
            )
            .unwrap();
        let variant = render(&source, VariantSpec::parse("20x20").unwrap()).unwrap();
        assert_eq!(variant.content_type.as_str(), "image/png");
        assert_eq!(
            ContentType::sniff(&variant.bytes).unwrap().as_str(),
            "image/png"
        );
    }

    #[test]
    fn a_declared_bomb_is_refused_before_it_is_decoded() {
        // A valid PNG header claiming 40000x40000 and no image data at all.
        // Fifty kilobytes of this would decode to 6.4 GB; it never gets that
        // far, because the header is read and refused on its own.
        let bomb = png_header(40_000, 40_000);
        assert!(matches!(
            render(&bomb, VariantSpec::parse("10x10").unwrap()),
            Err(StorageError::ImageTooLarge {
                width: 40_000,
                height: 40_000
            })
        ));
        // And one that is inside the per-axis cap but past the pixel budget.
        let wide = png_header(19_000, 19_000);
        assert!(matches!(
            render(&wide, VariantSpec::parse("10x10").unwrap()),
            Err(StorageError::ImageTooLarge { .. })
        ));
    }

    #[test]
    fn a_non_image_is_refused_before_a_decoder_sees_it() {
        for bytes in [
            &b"<!DOCTYPE html><script>alert(1)</script>"[..],
            &b"%PDF-1.7"[..],
            &b""[..],
            &b"\x1f\x8b\x08"[..],
        ] {
            assert!(matches!(
                render(bytes, VariantSpec::parse("10x10").unwrap()),
                Err(StorageError::NotAnImage)
            ));
        }
    }

    #[test]
    fn a_truncated_image_is_an_error_and_not_a_panic() {
        let source = png(40, 40);
        let truncated = &source[..source.len() / 2];
        assert!(matches!(
            render(truncated, VariantSpec::parse("10x10").unwrap()),
            Err(StorageError::Image(_))
        ));
    }

    #[test]
    fn a_variant_is_computed_once_and_then_read_from_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalDisk::open(dir.path()).unwrap();
        let key = store
            .put_blocking(&png(200, 100), &ObjectInfo::new())
            .unwrap();
        let spec = VariantSpec::parse("50x50").unwrap();

        let first = block_on(derive_variant(&store, &key, spec)).unwrap();
        assert_eq!(dimensions(&first.bytes), (50, 50));
        assert!(store.get_variant_blocking(&key, spec).unwrap().is_some());

        // Remove the source. A second call that still answers proves the
        // second call did not decode anything.
        std::fs::remove_file(
            store
                .root()
                .join("objects")
                .join(&key.to_hex()[0..2])
                .join(&key.to_hex()[2..4])
                .join(key.to_hex()),
        )
        .unwrap();
        let second = block_on(derive_variant(&store, &key, spec)).unwrap();
        assert_eq!(second.bytes, first.bytes);
        assert_eq!(second.content_type.as_str(), "image/jpeg");
    }

    #[test]
    fn deriving_from_nothing_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalDisk::open(dir.path()).unwrap();
        let spec = VariantSpec::parse("10x10").unwrap();
        assert!(matches!(
            block_on(derive_variant(&store, &ObjectKey::random().unwrap(), spec)),
            Err(StorageError::NotFound)
        ));
    }

    /// A structurally complete PNG that declares a huge size and carries
    /// almost no data. This is the decompression bomb in its cheapest form:
    /// the header promises billions of pixels and the file is a hundred bytes.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut out = Vec::new();
        out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(
            &mut out,
            b"IDAT",
            &[0x78, 0x01, 0x01, 0x00, 0x00, 0xff, 0xff],
        );
        chunk(&mut out, b"IEND", &[]);
        out
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
        let mut framed = Vec::with_capacity(4 + body.len());
        framed.extend_from_slice(kind);
        framed.extend_from_slice(body);
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&framed);
        out.extend_from_slice(&crc32(&framed).to_be_bytes());
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
}

/// The Postgres backend, against a real server.
///
/// # The test story, stated plainly
///
/// These cannot run with no server, and this crate's other fifty-three tests
/// must keep running on a laptop with nothing installed. So the suite is
/// compiled only with `--features postgres` and runs only when
/// `BROCANTH_TEST_DATABASE_URL` names a database it may create two tables in:
///
/// ```sh
/// BROCANTH_TEST_DATABASE_URL=postgres://localhost/brocanth_storage_test \
///   cargo test -p brocanth-storage --features postgres
/// ```
///
/// Not a testcontainer. That would put `testcontainers`, `bollard`, `hyper`
/// and a Docker daemon between `cargo test` and an answer, in the one crate
/// whose whole argument is that a feature which is off means the code is
/// absent — and it would still fail, less legibly, on a machine with no
/// Docker. An environment variable costs nothing and `brocanth-jobs` already
/// skips its Postgres suite the same way, so there is one convention to learn.
///
/// **A skip is loud.** [`announce_skip`] writes a banner straight to the
/// standard error handle, going round libtest's capture, because libtest
/// throws away what a *passing* test printed and a passing test is exactly the
/// case this message is about. A suite that passes by running nothing is worse
/// than one that is honestly empty.
///
/// What runs without a server is the statement-text suite in
/// [`crate::postgres::sql`]: that the primary key is `bytea`, that `size` is a
/// column rather than a detoast, that the foreign key cascades, and that no
/// insert in this backend can ever rewrite a row. Those are the properties the
/// design rests on, and they are checked on every `cargo test --all-features`
/// whether or not a database was named.
///
/// One database serves every test here and they run in parallel, so nothing
/// truncates anything. Each test works on its own addresses instead, which is
/// also what two applications sharing a database would do.
#[cfg(feature = "postgres")]
mod pg {
    use super::*;
    use crate::postgres::{Postgres, DEFAULT_MAX_OBJECT_BYTES, OBJECT_SIZE_CEILING};

    /// Says, once and where it cannot be swallowed, that the suite did not
    /// run.
    fn announce_skip() {
        use std::io::Write as _;
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // Not `eprintln!`. libtest redirects that macro and discards what
            // a passing test wrote; writing to the handle itself does not go
            // through the redirect.
            let _ = std::io::stderr().write_all(
                b"\n\
                  ============================================================\n\
                  SKIPPED: the brocanth-storage Postgres tests did not run.\n\
                  Nothing below was checked against a server.\n\
                  Set BROCANTH_TEST_DATABASE_URL to a database this suite may\n\
                  create tables in, for example:\n\
                    BROCANTH_TEST_DATABASE_URL=postgres://localhost/brocanth_storage_test\n\
                  ============================================================\n\n",
            );
        });
    }

    /// A reactor. `crate::rt::block_on` drives a future on the calling thread
    /// and has no I/O driver, so it cannot drive a socket.
    fn reactor() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a reactor")
    }

    /// Runs `body` against a migrated store, or announces a skip and does
    /// nothing.
    fn with_store<F>(body: F)
    where
        F: AsyncFnOnce(Postgres),
    {
        let Ok(url) = std::env::var("BROCANTH_TEST_DATABASE_URL") else {
            announce_skip();
            return;
        };
        reactor().block_on(async move {
            let store = Postgres::connect(&url).await.expect("connecting");
            store.migrate().await.expect("creating the tables");
            body(store).await;
        });
    }

    /// Bytes no other test in this file will store, so one database serves
    /// them all with no truncation and no ordering.
    fn unique(what: &str) -> Vec<u8> {
        let key = ObjectKey::random().expect("randomness");
        format!("brocanth-storage test {what} {key}").into_bytes()
    }

    #[test]
    fn an_object_round_trips_and_delete_is_idempotent() {
        with_store(async |store| {
            let body = unique("round trip");
            let about = ObjectInfo::new()
                .with_filename("contract.pdf")
                .with_declared_type("application/pdf");
            let key = store.put(&body, &about).await.expect("put");

            assert!(store.exists(&key).await.expect("exists"));
            assert_eq!(store.get(&key).await.expect("get"), body);

            let meta = store.meta(&key).await.expect("meta");
            assert_eq!(meta.key, key);
            assert_eq!(meta.size, body.len() as u64);
            // These bytes sniff as nothing, so the declared type is what is
            // left — and `is_safe_to_inline` still answers `false`, which is
            // the bit the serving half needs.
            assert_eq!(meta.content_type.as_str(), "application/pdf");
            assert!(!meta.content_type.is_safe_to_inline());
            assert_eq!(meta.filename.unwrap().as_str(), "contract.pdf");

            store.delete(&key).await.expect("delete");
            assert!(!store.exists(&key).await.expect("exists"));
            assert!(matches!(store.get(&key).await, Err(StorageError::NotFound)));
            assert!(matches!(
                store.meta(&key).await,
                Err(StorageError::NotFound)
            ));
            // Idempotent, because a delete retried after a timeout must not
            // fail the second time.
            store.delete(&key).await.expect("delete again");
            store.delete(&key).await.expect("delete a third time");
        });
    }

    #[test]
    fn the_content_decides_the_media_type() {
        with_store(async |store| {
            let mut body = b"\x89PNG\r\n\x1a\n".to_vec();
            body.extend_from_slice(&unique("sniffing"));
            let about = ObjectInfo::new().with_declared_type("text/html");
            let key = store.put(&body, &about).await.expect("put");
            let meta = store.meta(&key).await.expect("meta");
            assert_eq!(meta.content_type.as_str(), "image/png");
            assert!(meta.content_type.is_safe_to_inline());
            store.delete(&key).await.expect("delete");
        });
    }

    #[test]
    fn identical_content_is_stored_once_and_keeps_the_first_metadata() {
        with_store(async |store| {
            let body = unique("the metadata rewrite attack");
            let mine = ObjectInfo::new()
                .with_filename("mine.pdf")
                .with_declared_type("application/pdf");
            let yours = ObjectInfo::new()
                .with_filename("yours.html")
                .with_declared_type("text/html");

            let a = store.put(&body, &mine).await.expect("first put");
            let b = store.put(&body, &yours).await.expect("second put");
            assert_eq!(a, b, "the same bytes must get the same address");

            // The attack: uploading a copy of somebody else's file must not
            // change what their file is served as. `ON CONFLICT DO NOTHING`
            // left the first row alone, so it does not.
            let meta = store.meta(&a).await.expect("meta");
            assert_eq!(meta.content_type.as_str(), "application/pdf");
            assert_eq!(meta.filename.unwrap().as_str(), "mine.pdf");

            store.delete(&a).await.expect("delete");
        });
    }

    #[test]
    fn concurrent_puts_of_one_file_agree_and_still_refuse_the_rewrite() {
        with_store(async |store| {
            let body = std::sync::Arc::new(unique("concurrent puts"));
            let store = std::sync::Arc::new(store);
            let mine = ObjectInfo::new()
                .with_filename("mine.pdf")
                .with_declared_type("application/pdf");

            let mut tasks = Vec::new();
            for _ in 0..8 {
                let store = std::sync::Arc::clone(&store);
                let body = std::sync::Arc::clone(&body);
                let about = mine.clone();
                tasks.push(tokio::spawn(async move { store.put(&body, &about).await }));
            }
            let mut keys = Vec::new();
            for task in tasks {
                keys.push(task.await.expect("a task").expect("put"));
            }
            assert!(keys.windows(2).all(|pair| pair[0] == pair[1]));

            // Eight racing inserts, one row, no unique violation and nobody's
            // metadata overwritten — including by the late arrival below.
            let yours = ObjectInfo::new()
                .with_filename("yours.html")
                .with_declared_type("text/html");
            store.put(&body, &yours).await.expect("a late put");

            let meta = store.meta(&keys[0]).await.expect("meta");
            assert_eq!(meta.content_type.as_str(), "application/pdf");
            assert_eq!(meta.filename.unwrap().as_str(), "mine.pdf");
            assert_eq!(store.get(&keys[0]).await.expect("get"), *body);

            store.delete(&keys[0]).await.expect("delete");
        });
    }

    #[test]
    fn an_existing_address_is_never_overwritten() {
        with_store(async |store| {
            let key = ObjectKey::random().expect("randomness");
            let first = unique("first");
            let second = unique("second");
            store
                .put_at(&key, &first, &ObjectInfo::new())
                .await
                .expect("first");
            store
                .put_at(&key, &second, &ObjectInfo::new())
                .await
                .expect("second");
            assert_eq!(store.get(&key).await.expect("get"), first);
            store.delete(&key).await.expect("delete");
        });
    }

    #[test]
    fn a_keyed_store_addresses_differently_but_behaves_the_same() {
        with_store(async |store| {
            let body = unique("keyed addressing");
            let plain = store.address_of(&body);
            let keyed = Postgres::with_pool(store.pool().clone())
                .with_hash_key(HashKey::from_secret(b"deployment secret"));
            let key = keyed.put(&body, &ObjectInfo::new()).await.expect("put");
            assert_ne!(key, plain, "the key must change the address");
            assert_eq!(
                key,
                ObjectKey::keyed(&HashKey::from_secret(b"deployment secret"), &body)
            );
            assert_eq!(keyed.get(&key).await.expect("get"), body);
            // Dedupe still works under the key.
            assert_eq!(
                keyed.put(&body, &ObjectInfo::new()).await.expect("put"),
                key
            );
            keyed.delete(&key).await.expect("delete");
        });
    }

    #[test]
    fn a_variant_round_trips_and_dies_with_its_parent() {
        with_store(async |store| {
            let body = unique("a variant");
            let key = store.put(&body, &ObjectInfo::new()).await.expect("put");
            let spec: VariantSpec = "50x50t".parse().expect("a spec");

            assert_eq!(
                store.get_variant(&key, spec).await.expect("miss"),
                None,
                "a miss is ordinary control flow, not an error"
            );
            store
                .put_variant(&key, spec, b"thumb")
                .await
                .expect("put a variant");
            assert_eq!(
                store.get_variant(&key, spec).await.expect("hit"),
                Some(b"thumb".to_vec())
            );
            // Writing it twice is safe and leaves the cached bytes alone.
            store
                .put_variant(&key, spec, b"thumb")
                .await
                .expect("put it again");
            assert_eq!(
                store.get_variant(&key, spec).await.expect("hit"),
                Some(b"thumb".to_vec())
            );

            // A thumbnail of a deleted document is still the document, so the
            // foreign key cascades.
            store.delete(&key).await.expect("delete");
            assert_eq!(store.get_variant(&key, spec).await.expect("gone"), None);
        });
    }

    #[test]
    fn a_variant_of_an_object_that_is_not_there_is_refused() {
        with_store(async |store| {
            let key = ObjectKey::random().expect("randomness");
            let spec: VariantSpec = "10x10f".parse().expect("a spec");
            // The foreign key makes an orphan unrepresentable rather than
            // merely unlikely, so the honest answer is that the parent is
            // missing.
            assert!(matches!(
                store.put_variant(&key, spec, b"orphan").await,
                Err(StorageError::NotFound)
            ));
            assert_eq!(store.get_variant(&key, spec).await.expect("none"), None);
        });
    }

    #[test]
    fn an_object_past_the_limit_is_refused_before_it_is_sent() {
        with_store(async |store| {
            let small = Postgres::with_pool(store.pool().clone())
                .with_max_object_bytes(64)
                .expect("a limit");
            let body = vec![b'x'; 65];
            let key = small.address_of(&body);
            assert!(matches!(
                small.put(&body, &ObjectInfo::new()).await,
                Err(StorageError::TooLarge {
                    size: 65,
                    limit: 64
                })
            ));
            // Refused, not half-stored.
            assert!(!small.exists(&key).await.expect("exists"));

            // Exactly at the limit is stored.
            let fits = vec![b'y'; 64];
            let key = small.put(&fits, &ObjectInfo::new()).await.expect("put");
            assert_eq!(small.meta(&key).await.expect("meta").size, 64);

            // A variant is capped too.
            let spec: VariantSpec = "10x10f".parse().expect("a spec");
            assert!(matches!(
                small.put_variant(&key, spec, &[b'z'; 65]).await,
                Err(StorageError::TooLarge { .. })
            ));
            small.delete(&key).await.expect("delete");
        });
    }

    #[test]
    fn the_trait_works_through_a_trait_object() {
        with_store(async |store| {
            let store: std::sync::Arc<dyn Storage> = std::sync::Arc::new(store);
            let body = unique("a trait object");
            let key = store.put(&body, &ObjectInfo::new()).await.expect("put");
            assert_eq!(store.get(&key).await.expect("get"), body);
            store.delete(&key).await.expect("delete");
        });
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        with_store(async |store| {
            store.migrate().await.expect("again");
            store.migrate().await.expect("and again");
        });
    }

    // -- these need no server ----------------------------------------------

    #[test]
    fn the_size_limit_refuses_a_nonsense_setting() {
        // `connect_lazy` opens no connection, so this needs no server. It does
        // need a reactor: `sqlx` builds its pool's idle reaper on construction
        // and panics outside a Tokio context.
        reactor().block_on(async {
            let configure = |bytes| {
                let pool = sqlx::postgres::PgPool::connect_lazy("postgres://localhost/unused")
                    .expect("a lazy pool");
                Postgres::with_pool(pool).with_max_object_bytes(bytes)
            };
            assert!(configure(0).is_err(), "a limit of zero stores nothing");
            assert!(configure(OBJECT_SIZE_CEILING + 1).is_err());
            assert!(configure(OBJECT_SIZE_CEILING).is_ok());
            assert!(configure(1).is_ok());
            assert_eq!(
                configure(DEFAULT_MAX_OBJECT_BYTES)
                    .expect("the default is within the ceiling")
                    .max_object_bytes(),
                DEFAULT_MAX_OBJECT_BYTES
            );
        });
    }
}
