use clap::{Parser, Subcommand};
use common::kv::key_value_client::KeyValueClient;
use common::kv::{GetRequest, SetRequest};
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "rusty-kv-client")]
#[command(about = "A client for the Distributed Key-Value Store")]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:50051")]
    addr: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Get a value as text
    Get { key: String },
    /// Set a text value
    Set { key: String, value: String },
    /// Upload a file (like an image) to the KV store
    Upload { key: String, path: PathBuf },
    /// Download a value to a file
    Download { key: String, output: PathBuf },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    println!("Connecting to {}...", cli.addr);
    let mut client = KeyValueClient::connect(cli.addr).await?;

    match cli.command {
        Commands::Set { key, value } => {
            let req = tonic::Request::new(SetRequest {
                key: key.clone(),
                value: value.into_bytes(),
            });
            let _res = client.set(req).await?;
            println!("✅ SET SUCCESS: '{}'", key);
        }
        Commands::Get { key } => {
            let req = tonic::Request::new(GetRequest { key: key.clone() });
            let response = client.get(req).await?.into_inner();
            
            if response.found {
                // Try to print as string, otherwise show byte count
                match String::from_utf8(response.value.clone()) {
                    Ok(s) => println!("📦 VALUE: {}", s),
                    Err(_) => println!("📦 VALUE: [Binary Data, {} bytes]", response.value.len()),
                }
            } else {
                println!("❌ KEY NOT FOUND: '{}'", key);
            }
        }
        Commands::Upload { key, path } => {
            let data = fs::read(&path)?;
            let req = tonic::Request::new(SetRequest {
                key: key.clone(),
                value: data.clone(),
            });
            client.set(req).await?;
            println!("✅ UPLOAD SUCCESS: '{}' ({} bytes)", key, data.len());
        }
        Commands::Download { key, output } => {
            let req = tonic::Request::new(GetRequest { key: key.clone() });
            let response = client.get(req).await?.into_inner();

            if response.found {
                fs::write(&output, response.value.clone())?;
                println!("✅ SAVED TO: {:?} ({} bytes)", output, response.value.len());
            } else {
                println!("❌ KEY NOT FOUND: '{}'", key);
            }
        }
    }

    Ok(())
}