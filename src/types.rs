use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RequestEvent(pub Value);

#[derive(Debug, Serialize)]
#[serde(untagged, rename_all = "camelCase")]
pub enum InvocationResponse {
    Status(StatusResponse),
    Error(ErrorResponse),
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    error_message: String,
    error_type: String,
}

#[derive(Debug, Serialize)]
#[serde(untagged, rename_all = "camelCase")]
pub enum InvocationNextResponse {
    ErrorResponse(ErrorResponse),
    EventResponse(EventResponse),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorRequest {
    error_message: String,
    error_type: String,
    stack_trace: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct EventResponse(pub Map<String, Value>);
