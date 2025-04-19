use std::{collections::HashMap, sync::Arc};

use log::debug;
use tokio::{
    process::Command,
    sync::{Mutex, mpsc, oneshot},
};
use types::{EventResponse, RequestEvent};

pub mod api;
pub mod types;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AWSRequestId(pub String);

pub async fn rambda_handler<G: RuntimeGenerator, I: Fn() -> String>(
    request_event: RequestEvent,
    request_chan: RequestChannel,
    response_map: ResponseMap,
    mut runtime_manager: RuntimeManager<G>,
    gen_id: I,
) -> EventResponse {
    let aws_request_id = AWSRequestId(gen_id());

    // runtime側にリクエストを送信
    while let Err(SendRequestEventToChannelError::FailedToSend) = request_chan
        .send_request(&aws_request_id, request_event.clone())
        .await
    {
        debug!("request channel is full, waiting for runtime to process");
        // runtimeが処理中の場合は、新しいruntimeを生成しておく
        runtime_manager.init().await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    debug!("request channel sent");
    response_map.add_new_request(aws_request_id.clone()).await;

    // runtime側からのレスポンスを待つ
    debug!("waiting for runtime response");
    let response = response_map.get_response(&aws_request_id).await.unwrap();
    debug!("response: {:?}", response);
    response
}

pub async fn invocation_next_handler(
    mut chan: RequestChannel,
) -> Option<(AWSRequestId, RequestEvent)> {
    // runtime側にリクエストを返信
    match chan.recv_request().await {
        Some(request_event) => Some(request_event),
        None => panic!("request channel closed, removing request"),
    }
}

pub async fn invocation_response_handler(
    map: ResponseMap,
    aws_request_id: AWSRequestId,
    event_response: EventResponse,
) -> Result<(), String> {
    // rambda側にレスポンスを送信
    map.send_response(aws_request_id.clone(), event_response)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub struct RequestChannel {
    tx: mpsc::Sender<(AWSRequestId, RequestEvent)>,
    rx: Arc<Mutex<mpsc::Receiver<(AWSRequestId, RequestEvent)>>>,
}

pub enum SendRequestEventToChannelError {
    FailedToSend,
    SenderAlreadyTaken,
    RequestIdNotFound,
}

impl Default for RequestChannel {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel(1);
        RequestChannel {
            tx,
            rx: Arc::new(Mutex::new(rx)),
        }
    }
}

impl RequestChannel {
    pub async fn send_request(
        &self,
        aws_request_id: &AWSRequestId,
        resp: RequestEvent,
    ) -> Result<(), SendRequestEventToChannelError> {
        match self.tx.try_send((aws_request_id.clone(), resp)) {
            Ok(_) => Ok(()),
            Err(_) => {
                // bufferが1なので、受信側が受信していない場合はErrになる
                Err(SendRequestEventToChannelError::FailedToSend)
            }
        }
    }

    pub async fn recv_request(&mut self) -> Option<(AWSRequestId, RequestEvent)> {
        self.rx.lock().await.recv().await
    }
}
impl Clone for RequestChannel {
    fn clone(&self) -> Self {
        RequestChannel {
            tx: self.tx.clone(),
            rx: self.rx.clone(),
        }
    }
}

pub struct ResponseMap {
    r_map: Arc<Mutex<HashMap<AWSRequestId, oneshot::Receiver<EventResponse>>>>,
    t_map: Arc<Mutex<HashMap<AWSRequestId, oneshot::Sender<EventResponse>>>>,
}
impl Clone for ResponseMap {
    fn clone(&self) -> Self {
        ResponseMap {
            r_map: self.r_map.clone(),
            t_map: self.t_map.clone(),
        }
    }
}
impl Default for ResponseMap {
    fn default() -> Self {
        ResponseMap {
            r_map: Arc::new(Mutex::new(HashMap::new())),
            t_map: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl ResponseMap {
    pub async fn add_new_request(&self, aws_request_id: AWSRequestId) {
        let (tx, rx) = oneshot::channel();
        self.r_map.lock().await.insert(aws_request_id.clone(), rx);
        self.t_map.lock().await.insert(aws_request_id, tx);
    }
    pub async fn get_response(&self, aws_request_id: &AWSRequestId) -> Option<EventResponse> {
        let rx = self.r_map.lock().await.remove(aws_request_id);

        if let Some(rx) = rx {
            (rx.await).ok()
        } else {
            None
        }
    }
    pub async fn send_response(
        &self,
        aws_request_id: AWSRequestId,
        response: EventResponse,
    ) -> Result<(), String> {
        let tx = self.t_map.lock().await.remove(&aws_request_id);
        if let Some(tx) = tx {
            tx.send(response)
                .map_err(|_| "Failed to send response".to_string())
        } else {
            Err("Sender already taken".to_string())
        }
    }
}

pub struct RuntimeManager<G: RuntimeGenerator> {
    generator: G,
    runtime_list: Arc<Mutex<RuntimeList>>,
    lifetime_ms: u64,
}
impl<G: RuntimeGenerator> Clone for RuntimeManager<G> {
    fn clone(&self) -> Self {
        Self {
            generator: self.generator.clone(),
            runtime_list: self.runtime_list.clone(),
            lifetime_ms: self.lifetime_ms,
        }
    }
}

pub struct RuntimeProcessGenerator {
    cmd: String,
    args: Vec<String>,
}

impl RuntimeProcessGenerator {
    pub fn new(cmd: impl Into<String>, args: Vec<impl Into<String>>) -> Self {
        Self {
            cmd: cmd.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

impl RuntimeGenerator for RuntimeProcessGenerator {
    async fn init(&self) -> Result<Runtime, String> {
        let start_time = chrono::Utc::now().timestamp_millis() as u64;

        let child = Command::new(self.cmd.as_str())
            .args(&self.args)
            .env("AWS_LAMBDA_RUNTIME_API", "localhost:9001")
            .spawn()
            .map_err(|e| format!("Failed to spawn process: {}", e))?;
        let runtime = Runtime::new(RuntimeId(child.id().unwrap().to_string()), start_time);
        debug!("spawned process: {:?}", runtime.id);

        Ok(runtime)
    }

    async fn kill(&self, runtime_id: &RuntimeId) -> Result<(), String> {
        Command::new("kill")
            .args(["-9", &runtime_id.0])
            .spawn()
            .map_err(|e| format!("Failed to kill process: {}", e))?
            .wait()
            .await
            .map_err(|e| format!("Failed to wait for process: {}", e))?;
        debug!("killed process: {:?}", runtime_id);
        Ok(())
    }

    fn clone(&self) -> Self {
        Self {
            cmd: self.cmd.clone(),
            args: self.args.clone(),
        }
    }
}

impl<G: RuntimeGenerator> RuntimeManager<G> {
    pub fn new(generator: G, lifetime_ms: u64) -> Self {
        Self {
            generator,
            runtime_list: Arc::new(Mutex::new(RuntimeList::new())),
            lifetime_ms,
        }
    }
    pub async fn gc(&mut self) {
        let expires = self.runtime_list.lock().await.0.clone();
        let lifetime = self.lifetime_ms;
        let expires = expires
            .iter()
            .filter(|r| r.start + lifetime < chrono::Utc::now().timestamp_millis() as u64);
        for runtime in expires {
            self.kill(&runtime.id).await;
        }
        // もしruntimeが一つもなければ、再度生成する
        if self.runtime_list.lock().await.0.is_empty() {
            self.init().await.unwrap();
        }
        debug!("process num: {:?}", self.runtime_list.lock().await.0.len());
    }

    pub async fn init(&mut self) -> Result<Runtime, String> {
        let runtime = self.generator.init().await?;
        self.runtime_list.lock().await.add(runtime.clone());
        Ok(runtime)
    }

    async fn kill(&mut self, runtime_id: &RuntimeId) {
        self.generator.kill(runtime_id).await.unwrap();
        self.runtime_list.lock().await.remove(runtime_id);
    }
}

pub trait RuntimeGenerator {
    fn init(&self) -> impl Future<Output = Result<Runtime, String>>;
    fn kill(&self, runtime_id: &RuntimeId) -> impl Future<Output = Result<(), String>>;
    fn clone(&self) -> Self;
}

#[derive(Clone)]
struct RuntimeList(Vec<Runtime>);

impl RuntimeList {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn add(&mut self, runtime: Runtime) {
        self.0.push(runtime);
    }

    fn remove(&mut self, runtime_id: &RuntimeId) {
        self.0.retain(|r| r.id != *runtime_id);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    id: RuntimeId,
    // time
    start: u64,
}
impl Runtime {
    pub fn new(id: RuntimeId, start: u64) -> Self {
        Self { id, start }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeId(pub String);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, Value};
    struct MockRuntimeGenerator {
        initd_runtimes: Vec<Runtime>,
    }
    impl RuntimeGenerator for MockRuntimeGenerator {
        async fn init(&self) -> Result<Runtime, String> {
            let len = self.initd_runtimes.len();
            let runtime = Runtime::new(RuntimeId(format!("runtime_{}", len)), 0);
            Ok(runtime)
        }
        async fn kill(&self, _runtime_id: &RuntimeId) -> Result<(), String> {
            Ok(())
        }
        fn clone(&self) -> Self {
            Self {
                initd_runtimes: self.initd_runtimes.clone(),
            }
        }
    }
    fn new_mock_gen_id(id: String) -> impl Fn() -> String {
        let id = id;
        move || id.clone()
    }
    #[tokio::test]
    async fn test_rambda_handler() {
        let request_map = RequestChannel::default();
        let response_map = ResponseMap::default();

        for i in 0..100 {
            let mut event = Map::new();
            event.insert(
                "key".to_string(),
                Value::String(format!("event_{}", i).to_string()),
            );
            let request_event = RequestEvent(event);

            let id = format!("test_{}", i);
            let manager = RuntimeManager::new(
                MockRuntimeGenerator {
                    initd_runtimes: vec![],
                },
                0,
            );
            let rambda_handler_result = rambda_handler(
                request_event.clone(),
                request_map.clone(),
                response_map.clone(),
                manager.clone(),
                new_mock_gen_id(id.clone()),
            );
            let aws_request_id = AWSRequestId(id.clone());
            let wait_invocation_next = invocation_next_handler(request_map.clone());
            let mut response = Map::new();
            response.insert(
                "key".to_string(),
                Value::String(format!("response_{}", i).to_string()),
            );
            let wait_invocation_response = invocation_response_handler(
                response_map.clone(),
                aws_request_id.clone(),
                EventResponse(response.clone()),
            );
            let (rambda_handler_result, wait_invocation_next, wait_invocation_response) = tokio::join!(
                rambda_handler_result,
                wait_invocation_next,
                wait_invocation_response
            );
            assert_eq!(rambda_handler_result, EventResponse(response));
            assert_eq!(wait_invocation_next, Some((aws_request_id, request_event)));
            assert_eq!(wait_invocation_response, Ok(()));
        }
    }
}
