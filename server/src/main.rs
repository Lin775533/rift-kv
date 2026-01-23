use clap::Parser;
use common::kv::key_value_client::KeyValueClient;
use common::kv::key_value_server::{KeyValue, KeyValueServer};
use common::kv::{
    AppendEntriesRequest, AppendEntriesResponse, GetRequest, GetResponse, SetRequest, SetResponse,
    VoteRequest, VoteResponse,
};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tonic::{transport::Server, Request, Response, Status};

// --- Data Structures ---

#[derive(Debug, Clone, PartialEq)]
enum Role {
    Follower,
    Candidate,
    Leader,
}

#[derive(Debug)]
struct RaftState {
    term: u64,
    role: Role,
    voted_for: Option<u64>,
    last_heartbeat: Instant,
}

impl Default for RaftState {
    fn default() -> Self {
        Self {
            term: 0,
            role: Role::Follower,
            voted_for: None,
            last_heartbeat: Instant::now(),
        }
    }
}

#[derive(Debug)]
pub struct MyKeyValueStore {
    store: Arc<DashMap<String, Vec<u8>>>,
    raft_state: Arc<Mutex<RaftState>>,
}

impl Default for MyKeyValueStore {
    fn default() -> Self {
        Self {
            store: Arc::new(DashMap::new()),
            raft_state: Arc::new(Mutex::new(RaftState::default())),
        }
    }
}

// --- RPC Implementation ---

#[tonic::async_trait]
impl KeyValue for MyKeyValueStore {
    async fn set(&self, request: Request<SetRequest>) -> Result<Response<SetResponse>, Status> {
        let req = request.into_inner();
        self.store.insert(req.key, req.value);
        println!("SET request received");
        Ok(Response::new(SetResponse { success: true }))
    }

    async fn get(&self, request: Request<GetRequest>) -> Result<Response<GetResponse>, Status> {
        let req = request.into_inner();
        match self.store.get(&req.key) {
            Some(value) => Ok(Response::new(GetResponse {
                value: value.clone(),
                found: true,
            })),
            None => Ok(Response::new(GetResponse {
                value: vec![],
                found: false,
            })),
        }
    }

    async fn append_entries(
        &self,
        request: Request<AppendEntriesRequest>,
    ) -> Result<Response<AppendEntriesResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.raft_state.lock().await;

        // Reset timeout because we heard from a leader
        state.last_heartbeat = Instant::now();
        
        if req.term >= state.term {
            state.term = req.term;
            state.role = Role::Follower;
            state.voted_for = None;
        }

        println!("Received Heartbeat from Leader: {} (Term: {})", req.leader_id, state.term);

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

        println!("Vote request from {} (Term: {})", req.candidate_id, req.term);

        if req.term > state.term {
            state.term = req.term;
            state.role = Role::Follower;
            state.voted_for = None;
        }

        let grant_vote = if req.term >= state.term 
            && (state.voted_for.is_none() || state.voted_for == Some(req.candidate_id)) 
        {
            state.voted_for = Some(req.candidate_id);
            state.last_heartbeat = Instant::now();
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

async fn run_heartbeat(raft_state: Arc<Mutex<RaftState>>, peers: Vec<String>, my_port: u16) {
    loop {
        tokio::time::sleep(Duration::from_millis(2000)).await;

        let state = raft_state.lock().await;
        if state.role != Role::Leader {
            continue; // Only Leaders send heartbeats
        }
        let term = state.term;
        drop(state); // Drop lock before network requests

        println!("I am Leader. Sending heartbeats...");

        for peer in &peers {
            let peer_addr = peer.clone();
            tokio::spawn(async move {
                if let Ok(mut client) = KeyValueClient::connect(peer_addr.clone()).await {
                    let _ = client.append_entries(tonic::Request::new(AppendEntriesRequest {
                        term,
                        leader_id: my_port as u64,
                    })).await;
                }
            });
        }
    }
}

async fn start_election(raft_state: Arc<Mutex<RaftState>>, my_port: u16, peers: Vec<String>) {
    let mut state = raft_state.lock().await;
    state.role = Role::Candidate;
    state.term += 1;
    state.voted_for = Some(my_port as u64);
    let term = state.term;
    let my_id = my_port as u64;
    println!("Starting Election for Term {}...", term);
    drop(state);

    let mut votes = 1; // Vote for self
    let total_peers = peers.len();

    for peer in peers {
        let peer_addr = peer.clone();
        if let Ok(mut client) = KeyValueClient::connect(peer_addr.clone()).await {
            let req = tonic::Request::new(VoteRequest { term, candidate_id: my_id });
            if let Ok(resp) = client.request_vote(req).await {
                if resp.into_inner().vote_granted {
                    votes += 1;
                }
            }
        }
    }

    if votes > (total_peers + 1) / 2 {
        let mut state = raft_state.lock().await;
        if state.role == Role::Candidate && state.term == term {
            state.role = Role::Leader;
            println!("*** I AM THE LEADER (Term {}) ***", state.term);
        }
    }
}

async fn run_election_monitor(raft_state: Arc<Mutex<RaftState>>, my_port: u16, peers: Vec<String>) {
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut state = raft_state.lock().await;

        if state.role == Role::Leader {
            continue;
        }

        // Randomize this in production (150-300ms)
        if state.last_heartbeat.elapsed() > Duration::from_millis(2000) {
            println!("TIMEOUT! Leader is dead. Starting election...");
            drop(state);
            start_election(raft_state.clone(), my_port, peers.clone()).await;
            
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
    
    let kv_store = MyKeyValueStore::default();
    let state_clone = kv_store.raft_state.clone();
    let state_clone2 = kv_store.raft_state.clone();
    
    println!("Node listening on {}", addr);

    // Spawn Background Tasks
    if !args.peers.is_empty() {
        let peers1 = args.peers.clone();
        let peers2 = args.peers.clone();
        let port = args.port;

        tokio::spawn(async move {
            run_election_monitor(state_clone, port, peers1).await;
        });

        tokio::spawn(async move {
            run_heartbeat(state_clone2, peers2, port).await;
        });
    }

    Server::builder()
        .add_service(KeyValueServer::new(kv_store))
        .serve(addr)
        .await?;

    Ok(())
}