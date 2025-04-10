use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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
    pub error_message: String,
    pub error_type: String,
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

#[derive(Debug, Serialize, PartialEq, Clone, Deserialize)]
pub struct RequestEvent(pub Map<String, Value>);

#[derive(Debug, Serialize, PartialEq, Deserialize)]
pub struct EventResponse(pub Map<String, Value>);
