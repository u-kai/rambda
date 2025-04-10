use axum::{
    Router,
    routing::{get, post},
};
// curl localhost:3000 --json '{"key": "value"}'
use rambda::api::{AppState, invocation_next, invocation_response, rambda};
#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/2018-06-01/runtime/invocation/next", get(invocation_next))
        .route("/", post(rambda))
        .route(
            "/2018-06-01/runtime/invocation/{aws_request_id}/response",
            post(invocation_response),
        )
        .route(
            "/2018-06-01/runtime/invocation/{aws_request_id}/error",
            post(invocation_response),
        )
        .with_state(AppState::new());
    let listener = tokio::net::TcpListener::bind("localhost:3000")
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}
//
//// run our app with hyper, listening globally on port 3000
////
