use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, mpsc, oneshot};
use types::{EventResponse, RequestEvent};

pub mod api;
pub mod types;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AWSRequestId(String);

// runtimeのGCは別のプロセスでやった方が良いかも
// なぜなら起動したらruntimeが必ずしもrambda_handler内で取り扱っているrequest_idを消費するとは限らないから
// idleなruntimeがないからruntimeを起動したが、起動中に別のruntimeがidleになり、nextを起動するといったことは全然ありうるから
// ただ、今思ったのはnextを叩いてrequest_idを割り当てたruntimeがどのruntimeかを知らないと、runtimeのGCってむずくない？
// でもnextを叩くときにruntime側は別にruntime_idなどは渡さないからどうしよう
// 定期的に今のリクエスト量と、ランタイムの数を見て、減らしていかないとダメなのかも？
// idleとbusyの状態を知る術が難しい。。。
// 例えばinvocation/responseを送ってきたruntimeに対して、idleな状態に直すとかはありかも
// そうなるとaws_request_idからruntime_idを取得する必要があるが、どうする？?
// 上に書いている通り、runtimeを起動したからといって、そのruntimeに対して必ずしもrequest_idのリクエストが割当たるのかはわからない
// そうなるとinvocation_nextやresponseの時にruntime側がruntime_idをrequest_idと一緒に送って欲しいけど、そういう使用ではなかった気がする
// runtime_idがruntime側から送信されないとなると、制限時間や他のメトリクスからruntimeを管理するしかない
// ので、そうする
// send_requestでruntimeがない場合はブロックされると思うので、tryしてダメだったらruntimeを生成するようにする
// runtimeのGCは制限時間を確認しながら、shutdownを送る
// find_idleとかもいらないってことかな?とりあえず送信してnextで待っているやつがいないのであればすぐにgenerateする感じで良いと思う
pub async fn rambda_handler<G: RuntimeGenerator, I: Fn() -> String>(
    request_event: RequestEvent,
    request_map: RequestMap,
    response_map: ResponseMap,
    mut runtime_manager: RuntimeManager<G>,
    gen_id: I,
) -> EventResponse {
    let aws_request_id = AWSRequestId(gen_id());

    request_map.add_new_request(aws_request_id.clone()).await;
    // runtime側にリクエストを送信
    while let Err(SendRequestEventToChannelError::FailedToSend) = request_map
        .send_request(&aws_request_id, request_event.clone())
        .await
    {
        // runtimeがないので新しく生成する
        runtime_manager.generate().await.unwrap();
    }

    response_map.add_new_request(aws_request_id.clone()).await;

    // runtime側からのレスポンスを待つ
    let response = response_map.get_response(&aws_request_id).await.unwrap();
    response
}

pub async fn invocation_next_handler(
    map: RequestMap,
    aws_request_id: &AWSRequestId,
) -> Option<RequestEvent> {
    // runtime側にリクエストを送信
    match map.get_response(aws_request_id).await {
        Some(request_event) => Some(request_event),
        None => None,
    }
}

pub async fn invocation_response_handler(
    map: ResponseMap,
    aws_request_id: &AWSRequestId,
    event_response: EventResponse,
) -> Result<(), String> {
    // runtime側からのレスポンスを待つ
    // rambda側にレスポンスを送信
    map.send_response(aws_request_id, event_response)
        .await
        .map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use serde_json::{Map, Value};

    use super::*;
    struct MockRuntimeGenerator {
        generated_runtimes: Vec<Runtime>,
    }
    impl RuntimeGenerator for MockRuntimeGenerator {
        async fn generate(&self) -> Result<Runtime, String> {
            let len = self.generated_runtimes.len();
            let runtime = Runtime::new(RuntimeId(format!("runtime_{}", len)), 8080 + len as u16);
            Ok(runtime)
        }
        async fn kill(&self, _runtime_id: &RuntimeId) -> Result<(), String> {
            Ok(())
        }
        fn clone(&self) -> Self {
            Self {
                generated_runtimes: self.generated_runtimes.clone(),
            }
        }
    }
    fn new_mock_gen_id(id: &str) -> impl Fn() -> String {
        let id = id.to_string();
        move || id.clone()
    }
    #[tokio::test]
    async fn test_rambda_handler() {
        let request_map = RequestMap::new();
        let response_map = ResponseMap::new();

        for i in 0..100 {
            let request_event =
                RequestEvent(Value::String(format!("request_event_{}", i).to_string()));
            let manager = RuntimeManager::new(MockRuntimeGenerator {
                generated_runtimes: vec![],
            });

            let id = format!("test_{}", i);
            let rambda_handler_result = rambda_handler(
                request_event.clone(),
                request_map.clone(),
                response_map.clone(),
                manager.clone(),
                new_mock_gen_id(&id),
            );
            let aws_request_id = AWSRequestId(id.clone());
            let wait_invocation_next =
                invocation_next_handler(request_map.clone(), &aws_request_id);
            let mut response = Map::new();
            response.insert(
                "key".to_string(),
                Value::String(format!("response_{}", i).to_string()),
            );
            let wait_invocation_response = invocation_response_handler(
                response_map.clone(),
                &aws_request_id,
                EventResponse(response.clone()),
            );
            let (rambda_handler_result, wait_invocation_next, wait_invocation_response) = tokio::join!(
                rambda_handler_result,
                wait_invocation_next,
                wait_invocation_response
            );
            assert_eq!(rambda_handler_result, EventResponse(response));
            assert_eq!(wait_invocation_next, Some(request_event));
            assert_eq!(wait_invocation_response, Ok(()));
        }
    }
}

pub struct RequestMap {
    t_map: Arc<Mutex<HashMap<AWSRequestId, mpsc::Sender<RequestEvent>>>>,
    r_map: Arc<Mutex<HashMap<AWSRequestId, mpsc::Receiver<RequestEvent>>>>,
}

pub enum SendRequestEventToChannelError {
    FailedToSend,
    SenderAlreadyTaken,
    RequestIdNotFound,
}
impl RequestMap {
    pub fn new() -> Self {
        RequestMap {
            r_map: Arc::new(Mutex::new(HashMap::new())),
            t_map: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    pub async fn add_new_request(&self, aws_request_id: AWSRequestId) {
        let (tx, rx) = mpsc::channel(1);
        self.t_map.lock().await.insert(aws_request_id.clone(), tx);
        self.r_map.lock().await.insert(aws_request_id, rx);
    }

    pub async fn send_request(
        &self,
        aws_request_id: &AWSRequestId,
        resp: RequestEvent,
    ) -> Result<(), SendRequestEventToChannelError> {
        if let Some(request_mpsc) = self.t_map.lock().await.get_mut(aws_request_id) {
            match request_mpsc.try_send(resp) {
                Ok(_) => Ok(()),
                Err(_) => {
                    // bufferが1なので、受信側が受信していない場合はErrになる
                    // 再度senderをセットする
                    Err(SendRequestEventToChannelError::FailedToSend)
                }
            }
        } else {
            Err(SendRequestEventToChannelError::RequestIdNotFound)
        }
    }

    pub async fn get_response(&self, aws_request_id: &AWSRequestId) -> Option<RequestEvent> {
        if let Some(rx) = self.r_map.lock().await.get_mut(aws_request_id) {
            match rx.recv().await {
                Some(resp) => Some(resp),
                None => {
                    self.r_map.lock().await.remove(aws_request_id);
                    None
                }
            }
        } else {
            None
        }
    }
}
impl Clone for RequestMap {
    fn clone(&self) -> Self {
        RequestMap {
            r_map: self.r_map.clone(),
            t_map: self.t_map.clone(),
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

impl ResponseMap {
    pub fn new() -> Self {
        ResponseMap {
            r_map: Arc::new(Mutex::new(HashMap::new())),
            t_map: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    pub async fn add_new_request(&self, aws_request_id: AWSRequestId) {
        let (tx, rx) = oneshot::channel();
        self.r_map.lock().await.insert(aws_request_id.clone(), rx);
        self.t_map.lock().await.insert(aws_request_id, tx);
    }
    pub async fn get_response(&self, aws_request_id: &AWSRequestId) -> Option<EventResponse> {
        let rx = self.r_map.lock().await.remove(aws_request_id);
        if let Some(rx) = rx {
            match rx.await {
                Ok(resp) => Some(resp),
                Err(_) => None,
            }
        } else {
            None
        }
    }
    pub async fn send_response(
        &self,
        aws_request_id: &AWSRequestId,
        response: EventResponse,
    ) -> Result<(), String> {
        let tx = self.t_map.lock().await.remove(aws_request_id);
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
}

impl<G: RuntimeGenerator> RuntimeManager<G> {
    pub fn new(generator: G) -> Self {
        Self {
            generator,
            runtime_list: Arc::new(Mutex::new(RuntimeList::new())),
        }
    }
    pub fn clone(&self) -> Self {
        Self {
            generator: self.generator.clone(),
            runtime_list: self.runtime_list.clone(),
        }
    }

    async fn generate(&mut self) -> Result<Runtime, String> {
        let runtime = self.generator.generate().await?;
        self.runtime_list.lock().await.add(runtime.clone());
        Ok(runtime)
    }

    //fn find_idle(&self) -> Option<Runtime> {
    //    let runtime_list = self.runtime_list.lock().unwrap();
    //    runtime_list.find_idle().cloned()
    //}
    //fn idle(&mut self, runtime_id: &RuntimeId) {
    //    let mut runtime_list = self.runtime_list.lock().unwrap();
    //    if let Some(runtime) = runtime_list.0.iter_mut().find(|r| r.id == *runtime_id) {
    //        runtime.set_status(RuntimeStatus::Idle);
    //    }
    //}

    async fn kill(&mut self, runtime_id: &RuntimeId) {
        self.generator.kill(runtime_id).await;
        self.runtime_list.lock().await.remove(runtime_id);
    }
}

pub trait RuntimeGenerator {
    async fn generate(&self) -> Result<Runtime, String>;
    async fn kill(&self, runtime_id: &RuntimeId) -> Result<(), String>;
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

    //fn find_idle(&self) -> Option<&Runtime> {
    //    self.0.iter().find(|r| r.status == RuntimeStatus::Idle)
    //}

    fn remove(&mut self, runtime_id: &RuntimeId) {
        self.0.retain(|r| r.id != *runtime_id);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    id: RuntimeId,
    port: u16,
}
impl Runtime {
    pub fn new(id: RuntimeId, port: u16) -> Self {
        Self { id, port }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeId(String);

//#[derive(Debug, Clone, PartialEq, Eq)]
//enum RuntimeStatus {
//    Busy,
//    Idle,
//}
