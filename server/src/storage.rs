use rocksdb::{DB, Options};
use common::kv::LogEntry;
use prost::Message; // Needed for encoding/decoding Protobufs
use std::sync::Arc;
use std::convert::TryInto;

// A thread-safe wrapper around RocksDB
#[derive(Debug, Clone)]
pub struct Storage {
    db: Arc<DB>,
}

impl Storage {
    // Open the RocksDB database at the specified path
    pub fn new(path: &str) -> Self {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        
        let db = DB::open(&opts, path).expect("Failed to open RocksDB");
        
        Storage {
            db: Arc::new(db),
        }
    }

    // --- Raft Log Operations ---

    pub fn get_log_entry(&self, index: u64) -> Option<LogEntry> {
        let key = index.to_be_bytes(); // Convert index to bytes (Big Endian sorts correctly)
        match self.db.get(key) {
            Ok(Some(bytes)) => LogEntry::decode(&bytes[..]).ok(),
            _ => None,
        }
    }

    pub fn append_entry(&self, index: u64, entry: LogEntry) {
        let key = index.to_be_bytes();
        let value = entry.encode_to_vec(); // Convert Protobuf to raw bytes
        self.db.put(key, value).expect("Failed to write log entry to DB");
    }

    // Get the last log index (helper for startup)
    // Note: In a real system, you'd cache this or use an iterator. 
    // For now, we'll let the RaftState manage the index in memory after loading.

    // --- Hard State Operations (Term & Vote) ---

    pub fn save_hard_state(&self, term: u64, voted_for: Option<u64>) {
        self.db.put(b"current_term", term.to_be_bytes()).unwrap();
        match voted_for {
            Some(v) => self.db.put(b"voted_for", v.to_be_bytes()).unwrap(),
            None => self.db.delete(b"voted_for").unwrap(),
        }
    }

    pub fn load_hard_state(&self) -> (u64, Option<u64>) {
        let term = match self.db.get(b"current_term") {
            Ok(Some(bytes)) => u64::from_be_bytes(bytes.as_slice().try_into().unwrap()),
            _ => 0,
        };

        let voted_for = match self.db.get(b"voted_for") {
            Ok(Some(bytes)) => Some(u64::from_be_bytes(bytes.as_slice().try_into().unwrap())),
            _ => None,
        };

        (term, voted_for)
    }

    // --- State Machine (The actual User Data) ---
    // We prefix user keys with "data_" so they don't collide with Raft metadata or logs.
    
    pub fn apply_kv(&self, key: &str, value: &[u8]) {
        let db_key = format!("data_{}", key);
        self.db.put(db_key.as_bytes(), value).unwrap();
    }

    pub fn get_kv(&self, key: &str) -> Option<Vec<u8>> {
        let db_key = format!("data_{}", key);
        match self.db.get(db_key.as_bytes()) {
            Ok(Some(val)) => Some(val),
            _ => None,
        }
    }
}