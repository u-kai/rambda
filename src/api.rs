use axum::{Json, body::Body, extract::Path, http::HeaderMap};
use serde_json::Map;

use crate::types::{EventResponse, InvocationNextResponse, InvocationResponse, StatusResponse};

pub async fn invocation_next(headers: HeaderMap) -> Json<InvocationNextResponse> {
    let mut res = Map::new();
    res.insert("greet".to_string(), "Hello, World!".into());

    Json(InvocationNextResponse::EventResponse(EventResponse(res)))
}

struct InvocationNextHeaders {
    lambda_runtime_aws_request_id: String,
    lambda_runtime_trace_id: String,
    lambda_runtime_client_context: String,
    lambda_runtime_cognito_identity: String,
    lambda_runtime_deadline_ms: String,
    lambda_runtime_invoked_function_arn: String,
}

fn parse_header(headers: HeaderMap) -> InvocationNextHeaders {
    let lambda_runtime_aws_request_id = headers
        .get("lambda-runtime-aws-request-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let lambda_runtime_trace_id = headers
        .get("lambda-runtime-trace-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let lambda_runtime_client_context = headers
        .get("lambda-runtime-client-context")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let lambda_runtime_cognito_identity = headers
        .get("lambda-runtime-cognito-identity")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let lambda_runtime_deadline_ms = headers
        .get("lambda-runtime-deadline-ms")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let lambda_runtime_invoked_function_arn = headers
        .get("lambda-runtime-invoked-function-arn")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    InvocationNextHeaders {
        lambda_runtime_aws_request_id,
        lambda_runtime_trace_id,
        lambda_runtime_client_context,
        lambda_runtime_cognito_identity,
        lambda_runtime_deadline_ms,
        lambda_runtime_invoked_function_arn,
    }
}

pub async fn invocation_response(
    Path(aws_request_id): Path<String>,
    body: Body,
) -> Json<InvocationResponse> {
    println!("aws_request_id: {}", aws_request_id);

    Json(InvocationResponse::Status(StatusResponse {
        status: "OK".to_string(),
    }))
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
