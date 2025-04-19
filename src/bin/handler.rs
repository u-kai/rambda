use lambda_runtime::{Error, LambdaEvent, service_fn};
use log::debug;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug)]
struct Request {
    #[serde(default)]
    key: String,
}

#[derive(Serialize)]
struct Response {
    message: String,
}

async fn handler(event: LambdaEvent<Request>) -> Result<Response, Error> {
    let name = event.payload.key.clone();
    let message = if name.is_empty() {
        "Hello from Rust!".to_string()
    } else {
        format!("Hello, {}!", name)
    };
    debug!("event: {:?}", event);

    Ok(Response { message })
}

// TODO: なぜかうまくレスポンスを返してくれないので調査
#[tokio::main]
async fn main() -> Result<(), Error> {
    lambda_runtime::run(service_fn(handler)).await
}
