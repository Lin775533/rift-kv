use common::kv::key_value_client::KeyValueClient;
use common::kv::{GetRequest, SetRequest};
//use tonic::{Response, client};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>>{
    let mut client = KeyValueClient::connect("http://127.0.0.1:50051").await?;
    let set_req = tonic::Request::new(SetRequest{
        key: "testkey".into(),
        value: "HelloDistributed World".into(),
    });
    client.set(set_req).await?;
    println!("Written data to server");

    let get_req = tonic::Request::new(GetRequest{
        key: "testkey".into(),
    });
    let response = client.get(get_req).await?;
    let inner = response.into_inner();

    let val_string = String::from_utf8(inner.value).unwrap_or("Invalid UTF8".into());
    println!("Read from server: '{}' (found = {}", val_string, inner.found);

    Ok(())
}
