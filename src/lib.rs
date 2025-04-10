use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, mpsc, oneshot};
use types::{EventResponse, RequestEvent};

pub mod api;
pub mod types;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AWSRequestId(pub String);

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
        println!("request channel is full, waiting for runtime to process");
        // runtimeがないので新しく生成する
        runtime_manager.generate().await.unwrap();
    }

    println!("request channel sent");
    response_map.add_new_request(aws_request_id.clone()).await;

    // runtime側からのレスポンスを待つ
    println!("waiting for response");
    let response = response_map.get_response(&aws_request_id).await.unwrap();
    println!("response: {:?}", response);
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
    // runtime側からのレスポンスを待つ
    // rambda側にレスポンスを送信
    map.send_response(aws_request_id.clone(), event_response)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
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
            let runtime = Runtime::new(RuntimeId(format!("runtime_{}", len)), 8080 + len as u16, 0);
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
    fn new_mock_gen_id(id: String) -> impl Fn() -> String {
        let id = id;
        move || id.clone()
    }
    #[tokio::test]
    async fn test_rambda_handler() {
        let request_map = RequestChannel::new();
        let response_map = ResponseMap::new();

        for i in 0..100 {
            let mut event = Map::new();
            event.insert(
                "key".to_string(),
                Value::String(format!("event_{}", i).to_string()),
            );
            let request_event = RequestEvent(event);

            let id = format!("test_{}", i);
            let manager = RuntimeManager::new(MockRuntimeGenerator {
                generated_runtimes: vec![],
            });
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

pub struct RequestChannel {
    tx: mpsc::Sender<(AWSRequestId, RequestEvent)>,
    rx: Arc<Mutex<mpsc::Receiver<(AWSRequestId, RequestEvent)>>>,
}

pub enum SendRequestEventToChannelError {
    FailedToSend,
    SenderAlreadyTaken,
    RequestIdNotFound,
}
impl RequestChannel {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(1);
        RequestChannel {
            tx,
            rx: Arc::new(Mutex::new(rx)),
        }
    }

    pub async fn send_request(
        &self,
        aws_request_id: &AWSRequestId,
        resp: RequestEvent,
    ) -> Result<(), SendRequestEventToChannelError> {
        match self.tx.try_send((aws_request_id.clone(), resp)) {
            Ok(_) => Ok(()),
            Err(_) => {
                // bufferが1なので、受信側が受信していない場合はErrになる
                // 再度senderをセットする
                Err(SendRequestEventToChannelError::FailedToSend)
            }
        }
    }

    pub async fn recv_request(&mut self) -> Option<(AWSRequestId, RequestEvent)> {
        match self.rx.lock().await.recv().await {
            Some(resp) => Some(resp),
            None => None,
        }
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
}

pub struct RuntimeProcessGenerator {
    assign_ports: Vec<u16>,
}

impl RuntimeProcessGenerator {
    const BASE_PORT: u16 = 8080;
    pub fn new(assign_ports: Vec<u16>) -> Self {
        Self { assign_ports }
    }
}

impl RuntimeGenerator for RuntimeProcessGenerator {
    async fn generate(&self) -> Result<Runtime, String> {
        let port = Self::BASE_PORT + self.assign_ports.len() as u16;
        let start_time = chrono::Utc::now().timestamp_millis() as u64;
        let runtime = Runtime::new(RuntimeId(format!("runtime_{}", port)), port, start_time);

        Ok(runtime)
    }
    async fn kill(&self, _runtime_id: &RuntimeId) -> Result<(), String> {
        // ここで実際にRuntimeをkillする処理を書く
        Ok(())
    }
    fn clone(&self) -> Self {
        Self {
            assign_ports: self.assign_ports.clone(),
        }
    }
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
    pub async fn gc_loop(&self) {
        //tokio::spawn(async move {
        //    loop {
        //        // runtimeのGC処理
        //        let mut runtime_list = self.clone().runtime_list.lock().await;
        //        // ここでruntimeのGC処理を行う
        //        // 例えば、idleなruntimeを削除するなど
        //        // runtime_list.remove_idle_runtimes();
        //        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        //    }
        //});
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
        self.generator.kill(runtime_id).await.unwrap();
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

    fn remove(&mut self, runtime_id: &RuntimeId) {
        self.0.retain(|r| r.id != *runtime_id);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    id: RuntimeId,
    port: u16,
    // time
    start: u64,
}
impl Runtime {
    pub fn new(id: RuntimeId, port: u16, start: u64) -> Self {
        Self { id, port, start }
    }
    fn is_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp_millis() as u64;
        now - self.start > 60 * 1000
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeId(pub String);
