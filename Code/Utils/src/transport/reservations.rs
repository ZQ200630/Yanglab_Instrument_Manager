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
    fn reservation_key(&self) -> String {
        // Windows ASRL<N>::INSTR and direct COM<N> are one physical port.
        // Do not uppercase USB serials or TCP/IP device identifiers.
        let upper = self.0.to_ascii_uppercase();
        if let Some(number) = upper
            .strip_prefix("ASRL")
            .and_then(|s| s.strip_suffix("::INSTR"))
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|n| *n != 0)
        {
            format!("serial://COM{number}")
        } else {
            self.0.clone()
        }
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
            .contains_key(&resource.reservation_key())
    }
    pub(crate) fn reserve(&self, resource: &CanonicalResource) -> DriverResult<Reservation> {
        static TOKEN: AtomicU64 = AtomicU64::new(1);
        let mut entries = self
            .0
            .lock()
            .map_err(|_| DriverError::Responsibility("reservation registry poisoned".into()))?;
        let key = resource.reservation_key();
        if entries.contains_key(&key) {
            return Err(DriverError::Busy(resource.0.clone()));
        }
        let token = TOKEN.fetch_add(1, Ordering::Relaxed);
        entries.insert(key.clone(), token);
        Ok(Reservation {
            book: self.clone(),
            key,
            token,
            released: false,
        })
    }
}
/// No Drop release: only confirmed native close relinquishes a live claim.
pub(crate) struct Reservation {
    book: ResourceBook,
    key: String,
    token: u64,
    released: bool,
}
impl Reservation {
    pub(crate) fn release(&mut self) {
        if self.released {
            return;
        }
        let mut entries = self.book.0.lock().unwrap_or_else(|e| e.into_inner());
        if entries.get(&self.key) == Some(&self.token) {
            entries.remove(&self.key);
        }
        self.released = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asrl_and_com_share_one_reservation() {
        let book = ResourceBook::isolated();
        let com = CanonicalResource("serial://COM12".into());
        let asrl = CanonicalResource("ASRL12::INSTR".into());
        let mut serial = book.reserve(&com).unwrap();
        assert!(book.is_reserved(&asrl));
        assert!(matches!(book.reserve(&asrl), Err(DriverError::Busy(_))));
        serial.release();
        let mut visa = book.reserve(&asrl).unwrap();
        assert!(book.is_reserved(&com));
        visa.release();
    }
}
