#[cfg(test)]
mod tests {
    use super::super::{
        leases::{DomainRef, Session},
        registry::new_id,
    };
    use super::*;
    #[test]
    fn orphan_files_also_count_toward_storage_capacity() {
        let dir = std::env::temp_dir().join(format!("yang-orphan-{}", new_id().unwrap()));
        let boot = "b".repeat(32);
        let s = Session::local("c".repeat(32), boot.clone()).unwrap();
        let d = DomainRef {
            kind: "device".into(),
            id: "d".repeat(32),
        };
        let mut store = ResultStore::open(&dir, boot).unwrap();
        for n in 0..256 {
            std::fs::write(dir.join(format!("{n:032x}.bin")), b"").unwrap();
        }
        assert!(store.put(&s, &d, b"one").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn bounded_chunks_and_wrong_domain_or_boot_never_reveal_result() {
        let dir = std::env::temp_dir().join(format!("yang-results-{}", new_id().unwrap()));
        let boot = "b".repeat(32);
        let s = Session::local("c".repeat(32), boot.clone()).unwrap();
        let domain = DomainRef {
            kind: "device".into(),
            id: "d".repeat(32),
        };
        let mut store = ResultStore::open(&dir, boot.clone()).unwrap();
        let result = store.put(&s, &domain, b"spectrum").unwrap();
        assert_eq!(
            store.read(&s, &domain, &result.id, 0, 8).unwrap().data_hex,
            "737065637472756d"
        );
        let bad = Session::local("e".repeat(32), "f".repeat(32)).unwrap();
        assert!(store.read(&bad, &domain, &result.id, 0, 1).is_err());
        assert!(store
            .read(
                &s,
                &DomainRef {
                    kind: "device".into(),
                    id: "1".repeat(32)
                },
                &result.id,
                0,
                1
            )
            .is_err());
        assert!(store.read(&s, &domain, &result.id, 0, 262145).is_err());
        assert!(store
            .put(&s, &domain, &vec![0; 16 * 1024 * 1024 + 1])
            .is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
// Opaque, bounded result files; GUI never supplies a path.
use super::{
    contracts::{valid_id, HostError},
    leases::{DomainRef, Session},
    registry::{new_id, write_atomic},
    verification::sha256_bytes,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
pub const MAX_RESULT: usize = 16 * 1024 * 1024;
pub const MAX_CHUNK: u32 = 256 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultMeta {
    pub id: String,
    pub size: u64,
    pub checksum: String,
    pub domain: DomainRef,
    pub boot_id: String,
    pub acquired_by: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    version: u32,
    values: Vec<ResultMeta>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultChunk {
    pub id: String,
    pub offset: u64,
    pub size: u64,
    pub checksum: String,
    pub data_hex: String,
}
pub struct ResultStore {
    root: PathBuf,
    boot: String,
    index: Index,
}
pub fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 15) as usize] as char);
    }
    result
}
pub fn unhex(text: &str) -> Result<Vec<u8>, HostError> {
    if text.len() % 2 != 0
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(error("Invalid hexadecimal chunk"));
    }
    Ok(text
        .as_bytes()
        .chunks_exact(2)
        .map(|b| {
            let n = |c: u8| if c <= b'9' { c - b'0' } else { c - b'a' + 10 };
            n(b[0]) * 16 + n(b[1])
        })
        .collect())
}
impl ResultStore {
    pub fn open(root: &Path, boot: String) -> Result<Self, HostError> {
        if !valid_id(&boot) {
            return Err(error("Invalid result boot identity"));
        };
        std::fs::create_dir_all(root).map_err(error)?;
        let path = root.join("index.json");
        let index = if path.exists() {
            if std::fs::metadata(&path).map_err(error)?.len() > 256 * 1024 {
                return Err(error("Result index exceeds limit"));
            }
            serde_json::from_slice::<Index>(&std::fs::read(&path).map_err(error)?).map_err(error)?
        } else {
            Index {
                version: 1,
                values: Vec::new(),
            }
        };
        if index.version != 1
            || index.values.len() > 256
            || index.values.iter().any(|v| {
                !valid_id(&v.id) || v.size > MAX_RESULT as u64 || v.domain.validate().is_err()
            })
        {
            return Err(error("Unsupported or invalid result index"));
        }
        Ok(Self {
            root: root.into(),
            boot,
            index,
        })
    }
    pub fn put(
        &mut self,
        session: &Session,
        domain: &DomainRef,
        bytes: &[u8],
    ) -> Result<ResultMeta, HostError> {
        domain.validate()?;
        if session.boot_id() != self.boot || bytes.len() > MAX_RESULT {
            return Err(error("Result boot mismatch or size exceeds 16 MiB"));
        }
        // Unindexed files from a failed index commit still consume disk space.
        let mut files = 0usize;
        let mut physical_bytes = 0u64;
        for entry in std::fs::read_dir(&self.root).map_err(error)? {
            let entry = entry.map_err(error)?;
            if entry.path().extension().is_some_and(|s| s == "bin") {
                files += 1;
                physical_bytes =
                    physical_bytes.saturating_add(entry.metadata().map_err(error)?.len());
                if files >= 256
                    || physical_bytes.saturating_add(bytes.len() as u64) > 128 * 1024 * 1024
                {
                    return Err(error("Result storage capacity reached, including orphan files; explicit archival required"));
                }
            }
        }
        if self.index.values.len() >= 256
            || self.index.values.iter().map(|v| v.size).sum::<u64>() + bytes.len() as u64
                > 128 * 1024 * 1024
        {
            return Err(error(
                "Result storage capacity reached; explicitly archive results",
            ));
        }
        let meta = ResultMeta {
            id: new_id()?,
            size: bytes.len() as u64,
            checksum: sha256_bytes(bytes)?,
            domain: domain.clone(),
            boot_id: self.boot.clone(),
            acquired_by: session.id().into(),
        };
        write_atomic(&self.root.join(format!("{}.bin", meta.id)), bytes)?;
        let mut index = Index {
            version: 1,
            values: self.index.values.clone(),
        };
        index.values.push(meta.clone());
        write_atomic(
            &self.root.join("index.json"),
            &serde_json::to_vec(&index).map_err(error)?,
        )?;
        self.index = index;
        Ok(meta)
    }
    pub fn read(
        &self,
        session: &Session,
        domain: &DomainRef,
        id: &str,
        offset: u64,
        length: u32,
    ) -> Result<ResultChunk, HostError> {
        domain.validate()?;
        if session.boot_id() != self.boot || !valid_id(id) || length == 0 || length > MAX_CHUNK {
            return Err(error("Invalid result identity, boot or chunk size"));
        }
        let meta = self
            .index
            .values
            .iter()
            .find(|meta| meta.id == id && meta.boot_id == self.boot && meta.domain == *domain)
            .ok_or_else(|| error("Result outside authorized domain"))?;
        if offset > meta.size {
            return Err(error("Result offset exceeds size"));
        }
        let size = (meta.size - offset).min(length as u64) as usize;
        let mut file = std::fs::File::open(self.root.join(format!("{id}.bin"))).map_err(error)?;
        if file.metadata().map_err(error)?.len() != meta.size {
            return Err(error("Stored result changed"));
        }
        file.seek(SeekFrom::Start(offset)).map_err(error)?;
        let mut bytes = vec![0; size];
        file.read_exact(&mut bytes).map_err(error)?;
        Ok(ResultChunk {
            id: id.into(),
            offset,
            size: meta.size,
            checksum: meta.checksum.clone(),
            data_hex: hex_bytes(&bytes),
        })
    }
}
fn error(e: impl std::fmt::Display) -> HostError {
    HostError::new("ResultUnavailable", e.to_string())
}
type Job = Box<dyn FnOnce(&mut ResultStore) + Send>;
pub struct ResultActor {
    jobs: std::sync::mpsc::SyncSender<Job>,
}
impl ResultActor {
    pub fn new(store: ResultStore) -> Result<Self, HostError> {
        let (jobs, rx) = std::sync::mpsc::sync_channel::<Job>(32);
        std::thread::Builder::new()
            .name("Host result storage".into())
            .spawn(move || {
                let mut store = store;
                while let Ok(job) = rx.recv() {
                    job(&mut store);
                }
            })
            .map_err(error)?;
        Ok(Self { jobs })
    }
    pub async fn call<T: serde::de::DeserializeOwned>(
        &self,
        job: Box<dyn FnOnce(&mut ResultStore) -> Result<serde_json::Value, HostError> + Send>,
    ) -> Result<T, HostError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.jobs
            .try_send(Box::new(move |store| {
                let _ = tx.send(job(store));
            }))
            .map_err(|_| error("Result queue full or unavailable"))?;
        serde_json::from_value(rx.await.map_err(error)??).map_err(error)
    }
}
