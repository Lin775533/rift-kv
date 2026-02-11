use clap::Parser;
use common::kv::key_value_client::KeyValueClient;
use common::kv::key_value_server::{KeyValue, KeyValueServer};
use common::kv::{
    AppendEntriesRequest, AppendEntriesResponse, GetRequest, GetResponse, LogEntry, SetRequest,
    SetResponse, VoteRequest, VoteResponse,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tonic::{transport::Server, Request, Response, Status};
use tokio::sync::mpsc; 
use rand::Rng; // For randomized timeouts

// --- STORAGE MODULE ---
mod storage;
use storage::Storage;

// --- Data Structures ---

#[derive(Debug, Clone, PartialEq)]
enum Role {
    Follower,
    Candidate,
    Leader,
}

#[derive(Debug)]
struct RaftState {
    // Persistent State (Cached in memory for speed, but backed by disk)
    term: u64,
    voted_for: Option<u64>,

    // Volatile State
    role: Role,
    last_heartbeat: Instant,

    // Volatile Index State
    commit_index: u64,
    last_applied: u64,

    // We removed 'log: Vec<LogEntry>'
    // We now rely on 'storage' for the actual log data.
    // We keep track of the index/term here for quick access.
    last_log_index: u64,
    last_log_term: u64,
}

#[derive(Debug)]
pub struct MyKeyValueStore {
    // Replaced DashMap with RocksDB Wrapper
    storage: Storage,
    raft_state: Arc<Mutex<RaftState>>,
    my_port: u16,
}

impl MyKeyValueStore {
    pub async fn new(port: u16) -> Self {
        // Each node gets its own DB folder: "db_50051", "db_50052", etc.
        let path = format!("db_{}", port);
        let storage = Storage::new(&path);

        // RECOVERY: Load state from disk
        let (term, voted_for) = storage.load_hard_state();

        // RECOVERY: In a real system, we would scan RocksDB to find the last_log_index.
        // For this checkpoint, we assume 0 or start fresh.
        let last_log_index = 0;
        let last_log_term = 0;

        println!("Recovered Node {}: Term={}, VotedFor={:?}", port, term, voted_for);

        Self {
            storage,
            raft_state: Arc::new(Mutex::new(RaftState {
                term,
                voted_for,
                role: Role::Follower,
                last_heartbeat: Instant::now(),
                commit_index: 0,
                last_applied: 0,
                last_log_index,
                last_log_term,
            })),
            my_port: port,
        }
    }
}

// --- RPC Implementation ---

#[tonic::async_trait]
impl KeyValue for MyKeyValueStore {
    async fn set(&self, request: Request<SetRequest>) -> Result<Response<SetResponse>, Status> {
            let req = request.into_inner();
            let mut state = self.raft_state.lock().await;

            if state.role != Role::Leader {
                return Err(Status::failed_precondition("I am not the Leader."));
            }

            // 1. Create Entry
            let new_index = state.last_log_index + 1;
            let entry = LogEntry {
                term: state.term,
                command: "SET".into(),
                key: req.key,
                value: req.value,
            };

            // 2. Write to Log (WAL) - So we don't lose it on crash
            self.storage.append_entry(new_index, entry.clone());

            // 3. Update Memory Metadata
            state.last_log_index = new_index;
            state.last_log_term = state.term;

            // --- THE FIX: Apply to State Machine (The Database) ---
            // actually save the data so "get" can find it!
            state.commit_index = new_index;
            self.storage.apply_kv(&entry.key, &entry.value);
            state.last_applied = new_index; 
            
            println!("Leader: Logged AND Applied entry at index {}", new_index);
            Ok(Response::new(SetResponse { success: true }))
        }

    async fn get(&self, request: Request<GetRequest>) -> Result<Response<GetResponse>, Status> {
        let req = request.into_inner();

        // Read directly from RocksDB (State Machine)
        match self.storage.get_kv(&req.key) {
            Some(value) => Ok(Response::new(GetResponse { value, found: true })),
            None => Ok(Response::new(GetResponse { value: vec![], found: false })),
        }
    }

    async fn append_entries(
        &self,
        request: Request<AppendEntriesRequest>,
    ) -> Result<Response<AppendEntriesResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.raft_state.lock().await;

        state.last_heartbeat = Instant::now();

        // 1. Handle Term Updates
        if req.term > state.term {
            state.term = req.term;
            state.role = Role::Follower;
            state.voted_for = None;
            // PERSIST: Save new term to disk
            self.storage.save_hard_state(state.term, state.voted_for);
        }

        // 2. Log Replication
        if !req.entries.is_empty() {
            let mut max_index = state.last_log_index;
            let mut current_idx = req.prev_log_index;

            for entry in req.entries {
                current_idx += 1;
                self.storage.append_entry(current_idx, entry.clone());
                max_index = current_idx;
                state.last_log_term = entry.term;
            }
            state.last_log_index = max_index;
            println!("Follower: Synced log up to index {}", state.last_log_index);
        }

        // 3. Apply to State Machine
        while state.last_applied < req.leader_commit {
            let next_apply = state.last_applied + 1;

            if next_apply <= state.last_log_index {
                // FETCH from Disk to Apply
                if let Some(entry) = self.storage.get_log_entry(next_apply) {
                    if entry.command == "SET" {
                        self.storage.apply_kv(&entry.key, &entry.value);
                        println!("Follower: Applied '{}' to RocksDB", entry.key);
                    }
                }
                state.last_applied = next_apply;
            } else {
                break;
            }
        }

        Ok(Response::new(AppendEntriesResponse {
            term: state.term,
            success: true,
        }))
    }

    async fn request_vote(
        &self,
        request: Request<VoteRequest>,
    ) -> Result<Response<VoteResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.raft_state.lock().await;

        // 1. Term Update
        if req.term > state.term {
            state.term = req.term;
            state.role = Role::Follower;
            state.voted_for = None;
            self.storage.save_hard_state(state.term, state.voted_for);
        }

        // 2. Vote Logic
        let grant_vote = if req.term >= state.term
            && (state.voted_for.is_none() || state.voted_for == Some(req.candidate_id))
        {
            state.voted_for = Some(req.candidate_id);
            state.last_heartbeat = Instant::now();

            // PERSIST: Save vote to disk immediately!
            self.storage.save_hard_state(state.term, state.voted_for);

            true
        } else {
            false
        };

        Ok(Response::new(VoteResponse {
            term: state.term,
            vote_granted: grant_vote,
        }))
    }
}

// --- Background Tasks ---

async fn run_heartbeat(
    raft_state: Arc<Mutex<RaftState>>,
    storage: Storage, // <--- CHANGED: Removed underscore so we can use it
    peers: Vec<String>,
    my_port: u16,
) {
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;

        let state = raft_state.lock().await;
        if state.role != Role::Leader {
            continue;
        }

        let term = state.term;
        let commit_index = state.commit_index;
        let last_idx = state.last_log_index;
        let last_term = state.last_log_term;

        drop(state);

        // --- THE FIX START ---
        // Fetch the very last log entry from disk to send to followers.
        // (In a real Raft, we would check what EACH peer is missing, 
        // but for this project, sending the latest entry is enough to sync).
        let mut entries = vec![];
        let mut prev_log_index = last_idx;
        let mut prev_log_term = last_term;

        if last_idx > 0 {
            if let Some(entry) = storage.get_log_entry(last_idx) {
                entries.push(entry);
                // If we are sending entry #5, the "previous" index is #4
                prev_log_index = last_idx - 1;
                // We should technically fetch the term of #4, but 0 is fine for this stage
                prev_log_term = 0; 
            }
        }
        // --- THE FIX END ---

        for peer in &peers {
            let peer_addr = peer.clone();
            let entries_clone = entries.clone();

            tokio::spawn(async move {
                if let Ok(mut client) = KeyValueClient::connect(peer_addr.clone()).await {
                    let _ = client
                        .append_entries(tonic::Request::new(AppendEntriesRequest {
                            term,
                            leader_id: my_port as u64,
                            prev_log_index, // Use the calculated previous index
                            prev_log_term,
                            entries: entries_clone,
                            leader_commit: commit_index,
                        }))
                        .await;
                }
            });
        }
    }
}

async fn start_election(
    raft_state: Arc<Mutex<RaftState>>, 
    storage: Storage, 
    my_port: u16, 
    peers: Vec<String>
) {
    let mut state = raft_state.lock().await;
    state.role = Role::Candidate;
    state.term += 1;
    state.voted_for = Some(my_port as u64);

    // PERSIST: Save election state immediately
    storage.save_hard_state(state.term, state.voted_for);

    let term = state.term;
    let my_id = my_port as u64;
    let last_log_index = state.last_log_index;
    let last_log_term = state.last_log_term;
    
    println!("Starting Election (Term {})...", term);
    drop(state); // Unlock state so we don't block heartbeats

    // --- PARALLEL VOTING LOGIC ---
    let (tx, mut rx) = mpsc::channel(100);
    
    // Spawn a task for EVERY peer
    for peer in peers.clone() {
        let tx = tx.clone();
        tokio::spawn(async move {
            // 1. Set a strict timeout so a dead node doesn't block us forever
            let timeout_duration = Duration::from_millis(200);
            
            // 2. Try to connect
            if let Ok(Ok(mut client)) = tokio::time::timeout(
                timeout_duration, 
                KeyValueClient::connect(peer.clone())
            ).await {
                
                let req = tonic::Request::new(VoteRequest {
                    term,
                    candidate_id: my_id,
                    last_log_index,
                    last_log_term,
                });

                // 3. Request Vote with timeout
                if let Ok(Ok(resp)) = tokio::time::timeout(
                    timeout_duration, 
                    client.request_vote(req)
                ).await {
                    if resp.into_inner().vote_granted {
                        let _ = tx.send(true).await; // Send "Vote Granted"
                    }
                }
            }
            // If anything fails, the task dies quietly, and we don't block.
        });
    }

    // Drop our copy of the sender so the receiver knows when all tasks are done
    drop(tx);

    let mut votes = 1; // Vote for self
    let total_peers = peers.len();
    let required_votes = (total_peers + 1) / 2 + 1; // Simple majority

    // Collect votes as they arrive
    while let Some(_) = rx.recv().await {
        votes += 1;
        if votes >= required_votes {
            // We have a majority!
            let mut state = raft_state.lock().await;
            
            // Check if we are still a Candidate and the term hasn't changed
            if state.role == Role::Candidate && state.term == term {
                state.role = Role::Leader;
                println!("*** I AM THE LEADER (Term {}) ***", state.term);
                
                // Optional: Send immediate heartbeat here to assert authority
            }
            break; // Stop waiting for other votes
        }
    }
}

async fn run_election_monitor(
    raft_state: Arc<Mutex<RaftState>>,
    storage: Storage,
    my_port: u16,
    peers: Vec<String>,
) {
    loop {
        let timeout = rand::thread_rng().gen_range(1500..3000);
        tokio::time::sleep(Duration::from_millis(timeout)).await;

        let mut state = raft_state.lock().await;
        if state.role == Role::Leader {
            continue;
        }

        if state.last_heartbeat.elapsed() > Duration::from_millis(timeout) {
            println!("TIMEOUT! Starting election...");
            drop(state);
            start_election(
                raft_state.clone(),
                storage.clone(),
                my_port,
                peers.clone(),
            )
            .await;

            let mut state = raft_state.lock().await;
            state.last_heartbeat = Instant::now();
        }
    }
}

// --- Main ---

#[derive(Parser, Debug, Clone)]
struct Args {
    #[arg(long, default_value_t = 50051)]
    port: u16,
    #[arg(long, value_delimiter = ',')]
    peers: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let addr = format!("127.0.0.1:{}", args.port).parse()?;

    // Initialize Store (recovers from Disk)
    let kv_store = MyKeyValueStore::new(args.port).await;

    let state_clone = kv_store.raft_state.clone();
    let state_clone2 = kv_store.raft_state.clone();
    let storage_clone = kv_store.storage.clone();
    let storage_clone2 = kv_store.storage.clone();

    println!("Node listening on {}", addr);

    if !args.peers.is_empty() {
        let peers1 = args.peers.clone();
        let peers2 = args.peers.clone();
        let port = args.port;

        tokio::spawn(async move {
            run_election_monitor(state_clone, storage_clone, port, peers1).await;
        });

        tokio::spawn(async move {
            run_heartbeat(state_clone2, storage_clone2, peers2, port).await;
        });
    }

    Server::builder()
        .add_service(KeyValueServer::new(kv_store))
        .serve(addr)
        .await?;

    Ok(())
}