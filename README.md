```markdown
# rusty-kv: Distributed Key-Value Store

**rusty-kv** is a fault-tolerant, distributed Key-Value store engineered in Rust. It implements the **Raft Consensus Algorithm** to ensure strong data consistency and high availability across a multi-node cluster. The system features leader election, log replication, and persistent storage using a Write-Ahead Log (WAL) architecture.

![Rust](https://img.shields.io/badge/Rust-1.75%2B-000000?style=flat&logo=rust)
![License](https://img.shields.io/badge/License-MIT-blue?style=flat)
![Status](https://img.shields.io/badge/Status-Active-success?style=flat)

---

## 🏗️ System Architecture

The system consists of a cluster of independent nodes communicating via gRPC. Each node maintains its own RocksDB instance for persistence.

```mermaid
graph TD
    Client[Client Application] -->|gRPC Set/Get| Leader[Node 1 (Leader)]
    
    subgraph Cluster [Raft Cluster]
        Leader -->|AppendEntries RPC| Follower1[Node 2 (Follower)]
        Leader -->|AppendEntries RPC| Follower2[Node 3 (Follower)]
    end

    subgraph Storage [Persistence Layer]
        Leader --- DB1[(RocksDB WAL)]
        Follower1 --- DB2[(RocksDB WAL)]
        Follower2 --- DB3[(RocksDB WAL)]
    end

    classDef node fill:#f9f9f9,stroke:#333,stroke-width:2px;
    class Client,Leader,Follower1,Follower2 node;

```

## 🔄 Request Workflow (Log Replication)

The following sequence demonstrates how a write request (`SET`) is replicated and committed to ensure fault tolerance.

```mermaid
sequenceDiagram
    autonumber
    participant Client
    participant Leader
    participant Followers
    participant Disk as Persistent Storage

    Note over Client, Leader: 1. Client Request
    Client->>Leader: SET key="image" value=[bytes]
    
    activate Leader
    Leader->>Disk: Write to Log (WAL)
    Disk-->>Leader: Persisted
    
    Note over Leader, Followers: 2. Replication
    Leader->>Followers: AppendEntries (Log Index N)
    activate Followers
    Followers->>Disk: Write to Log (WAL)
    Disk-->>Followers: Persisted
    Followers-->>Leader: Success (Ack)
    deactivate Followers

    Note over Leader, Disk: 3. Commit Phase
    Leader->>Leader: Check Majority (Quorum)
    Leader->>Disk: Apply to State Machine (DB)
    
    Leader-->>Client: Response: Success
    deactivate Leader

    Note over Leader, Followers: 4. Async Apply
    Leader->>Followers: Heartbeat (Commit Index = N)
    Followers->>Disk: Apply to State Machine (DB)

```

## 🚀 Key Features

* **Raft Consensus:** Implements Leader Election and Log Replication to handle network partitions and node failures.
* **Persistence:** Uses **RocksDB** for durable storage. The system survives total power loss and restarts.
* **Strong Consistency:** Guarantees linearizable reads and writes by routing requests through the elected Leader.
* **gRPC Communication:** High-performance inter-node communication using **Tonic** and **Protobuf**.
* **Binary Support:** Efficiently handles binary data, supporting image and file storage.

## 🛠️ Tech Stack

* **Language:** Rust
* **RPC Framework:** Tonic (gRPC)
* **Async Runtime:** Tokio
* **Storage Engine:** RocksDB
* **Serialization:** Prost (Protobuf)
* **CLI:** Clap

## 📦 Getting Started

### Prerequisites

* Rust & Cargo (`rustup`)
* Protobuf Compiler (`protoc`)
* LLVM/Clang (Required for RocksDB bindings)

### Installation

```bash
# Clone the repository
git clone [https://github.com/YOUR_USERNAME/rusty-kv.git](https://github.com/YOUR_USERNAME/rusty-kv.git)
cd rusty-kv

# Build the project
cargo build --release

```

## 🏃 Usage

### 1. Start the Cluster

Run three separate terminal instances to create a local 3-node cluster.

**Node 1 (Leader Candidate):**

```bash
cargo run --bin server -- --port 50051 --peers [http://127.0.0.1:50052](http://127.0.0.1:50052),[http://127.0.0.1:50053](http://127.0.0.1:50053)

```

**Node 2:**

```bash
cargo run --bin server -- --port 50052 --peers [http://127.0.0.1:50051](http://127.0.0.1:50051),[http://127.0.0.1:50053](http://127.0.0.1:50053)

```

**Node 3:**

```bash
cargo run --bin server -- --port 50053 --peers [http://127.0.0.1:50051](http://127.0.0.1:50051),[http://127.0.0.1:50052](http://127.0.0.1:50052)

```

### 2. Client Operations

Use the client CLI to interact with the cluster. Ensure you connect to the current Leader's port (usually 50051 initially).

**Set a Value:**

```bash
cargo run --bin client -- --addr [http://127.0.0.1:50051](http://127.0.0.1:50051) set my_key "Hello World"

```

**Get a Value:**

```bash
cargo run --bin client -- --addr [http://127.0.0.1:50051](http://127.0.0.1:50051) get my_key

```

**Upload/Download Binary Files:**

```bash
# Upload an image
cargo run --bin client -- --addr [http://127.0.0.1:50051](http://127.0.0.1:50051) upload profile_pic ./avatar.png

# Download the image (verifies data integrity)
cargo run --bin client -- --addr [http://127.0.0.1:50051](http://127.0.0.1:50051) download profile_pic ./restored_avatar.png

```

## 🧪 Fault Tolerance Demo

To verify the system's resilience:

1. **Write Data:** Upload a file to the Leader.
2. **Simulate Crash:** Kill **all** terminal processes (`Ctrl+C`).
3. **Recover:** Restart the cluster terminals.
4. **Verify:** Use the client to download the file from a **Follower** node. The data will remain available and consistent.

## 📂 Project Structure

* `common/`: Shared Protobuf definitions (`kv.proto`) and data structures.
* `server/`: Core Raft implementation.
* `main.rs`: Election logic, Heartbeats, and RPC handlers.
* `storage.rs`: RocksDB wrapper for WAL and State Machine.


* `client/`: CLI tool for testing and interaction.

## 📄 License

This project is licensed under the MIT License.

```

