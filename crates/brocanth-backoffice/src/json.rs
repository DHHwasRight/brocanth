//! A loopback through JSON: every call and every answer is encoded and
//! decoded again on its way. The proof that the contract can cross a wire.

use async_trait::async_trait;
use brocanth_admin::discovery::RegistryEntry;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::contract::{Answer, Call, Caller, Claim, TabData, TabError, TabTransaction, Write};

/// A [`TabData`] that sends each call to `T` as JSON text, and each answer
/// and error back the same way.
///
/// Nothing in one process needs it. It exists so that a test can run the
/// admin's screens with every call crossing a codec, which is what a
/// transport between two processes will do (docs/backoffice.md §7.1, D2).
/// A type that does not survive the trip fails here, not in production.
#[derive(Debug)]
pub struct Json<T> {
    inner: T,
}

impl<T: TabData> Json<T> {
    /// `inner`, behind the codec.
    pub fn new(inner: T) -> Json<T> {
        Json { inner }
    }
}

/// `value` as JSON text and back.
///
/// # Errors
///
/// [`TabError`] of kind `Protocol` when either half fails.
pub fn trip<V: Serialize + DeserializeOwned>(value: &V) -> Result<V, TabError> {
    let text = serde_json::to_string(value)
        .map_err(|e| TabError::protocol(format!("a value did not encode: {e}")))?;
    serde_json::from_str(&text)
        .map_err(|e| TabError::protocol(format!("a value did not decode: {e}: {text}")))
}

/// An answer or an error, after the trip.
fn back(answer: Result<Answer, TabError>) -> Result<Answer, TabError> {
    trip(&answer)?
}

#[async_trait]
impl<T: TabData> TabData for Json<T> {
    fn registry(&self) -> Vec<RegistryEntry> {
        // `registry` cannot fail, so a registry that does not survive the
        // codec stops the test that built the loopback.
        trip(&self.inner.registry()).unwrap_or_else(|e| panic!("{e}"))
    }

    fn check(&self) -> Result<(), TabError> {
        match self.inner.check() {
            Ok(()) => Ok(()),
            Err(error) => Err(trip(&error)?),
        }
    }

    async fn call(&self, call: Call) -> Result<Answer, TabError> {
        let call = trip(&call)?;
        back(self.inner.call(call).await)
    }

    async fn begin(&self, claim: &Claim) -> Result<Box<dyn TabTransaction>, TabError> {
        let claim = trip(claim)?;
        match self.inner.begin(&claim).await {
            Ok(tx) => Ok(Box::new(JsonTransaction { inner: tx })),
            Err(error) => Err(trip(&error)?),
        }
    }
}

/// A transaction behind the codec.
struct JsonTransaction {
    inner: Box<dyn TabTransaction>,
}

#[async_trait]
impl TabTransaction for JsonTransaction {
    async fn call(&mut self, caller: Caller, write: Write) -> Result<Answer, TabError> {
        let (caller, write) = trip(&(caller, write))?;
        back(self.inner.call(caller, write).await)
    }

    async fn commit(self: Box<Self>) -> Result<(), TabError> {
        match self.inner.commit().await {
            Ok(()) => Ok(()),
            Err(error) => Err(trip(&error)?),
        }
    }

    async fn rollback(self: Box<Self>) -> Result<(), TabError> {
        match self.inner.rollback().await {
            Ok(()) => Ok(()),
            Err(error) => Err(trip(&error)?),
        }
    }
}
