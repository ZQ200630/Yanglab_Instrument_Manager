use crate::{DriverError, DriverResult};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
};
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CanonicalResource(pub(crate) String);
impl CanonicalResource {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Clone)]
pub struct ResourceBook(Arc<Mutex<HashMap<String, u64>>>);
impl Default for ResourceBook {
    fn default() -> Self {
        static BOOK: OnceLock<ResourceBook> = OnceLock::new();
        BOOK.get_or_init(Self::isolated).clone()
    }
}
impl ResourceBook {
    /// Separate finite-test reservation namespace. Production uses Default.
    pub fn isolated() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }
    pub fn is_reserved(&self, resource: &CanonicalResource) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(resource.as_str())
    }
    pub(crate) fn reserve(&self, resource: &CanonicalResource) -> DriverResult<Reservation> {
        static TOKEN: AtomicU64 = AtomicU64::new(1);
        let mut entries = self
            .0
            .lock()
            .map_err(|_| DriverError::Responsibility("reservation registry poisoned".into()))?;
        if entries.contains_key(resource.as_str()) {
            return Err(DriverError::Busy(resource.0.clone()));
        }
        let token = TOKEN.fetch_add(1, Ordering::Relaxed);
        entries.insert(resource.0.clone(), token);
        Ok(Reservation {
            book: self.clone(),
            resource: resource.clone(),
            token,
            released: false,
        })
    }
}
/// No Drop release: only confirmed native close relinquishes a live claim.
pub(crate) struct Reservation {
    book: ResourceBook,
    resource: CanonicalResource,
    token: u64,
    released: bool,
}
impl Reservation {
    pub(crate) fn release(&mut self) {
        if self.released {
            return;
        }
        let mut entries = self.book.0.lock().unwrap_or_else(|e| e.into_inner());
        if entries.get(self.resource.as_str()) == Some(&self.token) {
            entries.remove(self.resource.as_str());
        }
        self.released = true;
    }
}
