use std::convert::Infallible;

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};

use crate::{
    RequestChannel, ResponseMap, Runtime, RuntimeGenerator, RuntimeManager,
    invocation_next_handler, invocation_response_handler, rambda_handler,
    types::{
        ErrorResponse, EventResponse, InvocationNextResponse, InvocationResponse, RequestEvent,
        StatusResponse,
    },
};

pub struct AppState<G: RuntimeGenerator> {
    request_chan: RequestChannel,
    response_map: ResponseMap,
    runtime_manager: RuntimeManager<G>,
}

pub struct MockRuntimeGenerator;

impl RuntimeGenerator for MockRuntimeGenerator {
    async fn generate(&self) -> Result<Runtime, String> {
        Ok(Runtime::new(crate::RuntimeId("hello".to_string()), 8000, 0))
    }
    async fn kill(&self, runtime_id: &crate::RuntimeId) -> Result<(), String> {
        Ok(())
    }
    fn clone(&self) -> Self {
        MockRuntimeGenerator
    }
}
impl Clone for MockRuntimeGenerator {
    fn clone(&self) -> Self {
        MockRuntimeGenerator
    }
}

impl AppState<MockRuntimeGenerator> {
    pub fn new() -> Self {
        AppState {
            request_chan: RequestChannel::new(),
            response_map: ResponseMap::new(),
            runtime_manager: RuntimeManager::new(MockRuntimeGenerator),
        }
    }
}
impl Clone for AppState<MockRuntimeGenerator> {
    fn clone(&self) -> Self {
        AppState {
            request_chan: self.request_chan.clone(),
            response_map: self.response_map.clone(),
            runtime_manager: self.runtime_manager.clone(),
        }
    }
}

//#[axum::debug_handler]
pub async fn rambda<G: RuntimeGenerator>(
    State(state): State<AppState<G>>,
    Json(event): Json<RequestEvent>,
) -> Json<EventResponse> {
    println!("event: {:?}", event);
    fn gen_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    let response = rambda_handler(
        event,
        state.request_chan,
        state.response_map,
        state.runtime_manager,
        gen_id,
    )
    .await;
    Json(response)
}

pub async fn invocation_next<G: RuntimeGenerator>(
    State(state): State<AppState<G>>,
) -> (HeaderMap, Json<InvocationNextResponse>) {
    println!("invocation_next");
    let response = invocation_next_handler(state.request_chan).await.unwrap();
    let mut header = HeaderMap::new();
    header.insert(
        "Lambda-Runtime-Aws-Request-Id",
        response.0.0.parse().unwrap(),
    );
    header.insert("Lambda-Runtime-Trace-Id", "trace-id".parse().unwrap());
    header.insert(
        "Lambda-Runtime-Client-Context",
        r#"{"key":"value"}"#.parse().unwrap(),
    );
    header.insert(
        "Lambda-Runtime-Cognito-Identity",
        r#"{"key":"value"}"#.parse().unwrap(),
    );
    header.insert(
        "Lambda-Runtime-Invoked-Function-Arn",
        "arn:aws:lambda:us-east-1:123456789012:function:my-function"
            .parse()
            .unwrap(),
    );
    header.insert("Lambda-Runtime-Deadline-Ms", "3000".parse().unwrap());
    (
        header,
        Json(InvocationNextResponse::EventResponse(EventResponse(
            response.1.0,
        ))),
    )
}

pub async fn invocation_response<G: RuntimeGenerator>(
    State(state): State<AppState<G>>,
    Path(aws_request_id): Path<String>,
    Json(event_response): Json<Option<EventResponse>>,
) -> (StatusCode, Json<InvocationResponse>) {
    println!("invocation response aws_request_id: {}", aws_request_id);
    let aws_request_id = crate::AWSRequestId(aws_request_id.clone());
    let send_result = invocation_response_handler(
        state.response_map,
        aws_request_id,
        event_response.unwrap_or(EventResponse(serde_json::from_str("{}").unwrap())),
    )
    .await;
    match send_result {
        Ok(_) => (
            StatusCode::ACCEPTED,
            Json(InvocationResponse::Status(StatusResponse {
                status: "OK".to_string(),
            })),
        ),
        Err(s) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(InvocationResponse::Error(ErrorResponse {
                error_message: s,
                error_type: "NoResponse".to_string(),
            })),
        ),
    }
}

pub async fn invocation_error(
    Path(aws_request_id): Path<String>,
    body: Body,
) -> Json<InvocationResponse> {
    println!("aws_request_id: {}", aws_request_id);

    Json(InvocationResponse::Status(StatusResponse {
        status: "OK".to_string(),
    }))
}
