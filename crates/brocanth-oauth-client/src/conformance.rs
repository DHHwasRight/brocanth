//! The [`TokenStore`] contract, as one async function.
//!
//! [`run`] checks every rule a store must keep, against the store it is
//! given. `MemoryTokenStore` and `PgTokenStore` both pass it, in this
//! crate's tests. A store written elsewhere calls it from its own test:
//!
//! ```ignore
//! #[tokio::test]
//! async fn my_store_keeps_the_contract() {
//!     brocanth_oauth_client::conformance::run(&MyStore::new()).await;
//! }
//! ```
//!
//! It writes only accounts that start with a random prefix, and deletes
//! nothing it did not write, so it can run against a shared table.
//! [`run`] panics on the first broken rule, with the rule in the message.

use crate::key::random_text;
use crate::store::{StoredToken, TokenStore};

fn row(account: &str, provider: &str, tag: u8) -> StoredToken {
    StoredToken {
        account: account.to_string(),
        provider: provider.to_string(),
        access: vec![1, tag, 0, 255],
        refresh: (tag % 2 == 0).then(|| vec![2, tag]),
        token_type: "Bearer".to_string(),
        scopes: vec!["read".to_string(), format!("s{tag}")],
        expires_at: Some(1_000 + i64::from(tag)),
        connected_at: 10,
        refreshed_at: 20 + i64::from(tag),
    }
}

/// Runs every rule against `store`. Panics on the first broken one.
pub async fn run<S: TokenStore>(store: &S) {
    let prefix = format!("conformance-{}", random_text(6));
    let alice = format!("{prefix}-alice");
    let bob = format!("{prefix}-bob");

    // A key nobody wrote reads as nothing.
    assert_eq!(
        store.get(&alice, "github").await.unwrap(),
        None,
        "get of a missing key must be None"
    );

    // What goes in comes out, byte for byte.
    let first = row(&alice, "github", 2);
    store.put(first.clone()).await.unwrap();
    assert_eq!(
        store.get(&alice, "github").await.unwrap().as_ref(),
        Some(&first),
        "get must return the row put wrote"
    );

    // A second put replaces the whole row: no field of the old one stays.
    let second = row(&alice, "github", 3);
    store.put(second.clone()).await.unwrap();
    assert_eq!(
        store.get(&alice, "github").await.unwrap().as_ref(),
        Some(&second),
        "put must replace the row whole (refresh too, here None)"
    );

    // Keys are (account, provider): neither half alone.
    store.put(row(&alice, "google", 4)).await.unwrap();
    store.put(row(&bob, "github", 6)).await.unwrap();
    assert_eq!(
        store.get(&bob, "google").await.unwrap(),
        None,
        "the key is the pair"
    );

    // An account's list holds its rows only, by provider, without tokens.
    let listed = store.for_account(&alice).await.unwrap();
    let providers: Vec<&str> = listed.iter().map(|c| c.provider.as_str()).collect();
    assert_eq!(
        providers,
        ["github", "google"],
        "for_account lists one account's rows, by provider"
    );
    assert!(!listed[0].has_refresh && listed[1].has_refresh);
    assert_eq!(listed[1].scopes, ["read", "s4"]);
    assert_eq!(listed[1].expires_at, Some(1_004));

    // The full list is ordered by account then provider, and pages.
    let mut all = Vec::new();
    let mut offset = 0;
    loop {
        let page = store.list(2, offset).await.unwrap();
        if page.is_empty() {
            break;
        }
        offset += page.len();
        all.extend(page);
    }
    let ours: Vec<(String, String)> = all
        .iter()
        .filter(|c| c.account.starts_with(&prefix))
        .map(|c| (c.account.clone(), c.provider.clone()))
        .collect();
    assert_eq!(
        ours,
        [
            (alice.clone(), "github".to_string()),
            (alice.clone(), "google".to_string()),
            (bob.clone(), "github".to_string()),
        ],
        "list orders by account then provider, and limit/offset page it"
    );

    // Delete says whether there was a row, and leaves the others.
    assert!(
        store.delete(&alice, "github").await.unwrap(),
        "delete of a row is true"
    );
    assert!(
        !store.delete(&alice, "github").await.unwrap(),
        "a second delete is false"
    );
    assert_eq!(store.get(&alice, "github").await.unwrap(), None);
    assert!(store.get(&alice, "google").await.unwrap().is_some());

    // Two writers of one key leave one whole row: one of the two.
    let writes = (10..30u8).map(|tag| store.put(row(&bob, "gitlab", tag)));
    for result in futures_join(writes).await {
        result.unwrap();
    }
    let kept = store.get(&bob, "gitlab").await.unwrap().expect("one row");
    assert!(
        (10..30u8).any(|tag| kept == row(&bob, "gitlab", tag)),
        "concurrent puts must leave one whole row, not a mix"
    );

    for (account, provider) in [(&alice, "google"), (&bob, "github"), (&bob, "gitlab")] {
        store.delete(account, provider).await.unwrap();
    }
}

/// Awaits every future, polled together.
async fn futures_join<F: std::future::Future>(
    futures: impl IntoIterator<Item = F>,
) -> Vec<F::Output> {
    let mut pending: Vec<_> = futures.into_iter().map(Box::pin).collect();
    let mut out: Vec<Option<F::Output>> = (0..pending.len()).map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut done = true;
        for (i, f) in pending.iter_mut().enumerate() {
            if out[i].is_none() {
                match f.as_mut().poll(cx) {
                    std::task::Poll::Ready(v) => out[i] = Some(v),
                    std::task::Poll::Pending => done = false,
                }
            }
        }
        if done {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
    out.into_iter().map(|v| v.expect("ready")).collect()
}
