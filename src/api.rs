use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use log::debug;

use crate::{
    RequestChannel, ResponseMap, RuntimeGenerator, RuntimeManager, RuntimeProcessGenerator,
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

impl AppState<RuntimeProcessGenerator> {
    pub fn new(manager: RuntimeManager<RuntimeProcessGenerator>) -> Self {
        AppState {
            request_chan: RequestChannel::default(),
            response_map: ResponseMap::default(),
            runtime_manager: manager,
        }
    }
}
impl Clone for AppState<RuntimeProcessGenerator> {
    fn clone(&self) -> Self {
        AppState {
            request_chan: self.request_chan.clone(),
            response_map: self.response_map.clone(),
            runtime_manager: self.runtime_manager.clone(),
        }
    }
}

pub async fn rambda<G: RuntimeGenerator>(
    State(state): State<AppState<G>>,
    Json(event): Json<RequestEvent>,
) -> Json<EventResponse> {
    debug!("rambda event: {:?}", event);

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
    debug!("invocation_next");

    let response = invocation_next_handler(state.request_chan).await.unwrap();
    let aws_request_id = response.0;
    let request_event = response.1;

    let mut header = HeaderMap::new();
    header.insert(
        "Lambda-Runtime-Aws-Request-Id",
        aws_request_id.0.parse().unwrap(),
    );
    header.insert("Lambda-Runtime-Trace-Id", "trace-id".parse().unwrap());
    header.insert(
        "Lambda-Runtime-Invoked-Function-Arn",
        "arn:aws:lambda:us-east-1:123456789012:function:my-function"
            .parse()
            .unwrap(),
    );
    header.insert("Lambda-Runtime-Deadline-Ms", "3000".parse().unwrap());

    (
        header,
        Json(InvocationNextResponse::EventResponse(request_event.into())),
    )
}

pub async fn invocation_response<G: RuntimeGenerator>(
    State(state): State<AppState<G>>,
    Path(aws_request_id): Path<String>,
    Json(event_response): Json<Option<EventResponse>>,
) -> (StatusCode, Json<InvocationResponse>) {
    debug!("invocation response aws_request_id: {}", aws_request_id);
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

// TODO
pub async fn invocation_error(
    Path(aws_request_id): Path<String>,
    _body: Body,
) -> Json<InvocationResponse> {
    debug!("aws_request_id: {}", aws_request_id);

    Json(InvocationResponse::Status(StatusResponse {
        status: "OK".to_string(),
    }))
}
