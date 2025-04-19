use axum::{
    Router,
    routing::{get, post},
};
use rambda::{
    RuntimeManager, RuntimeProcessGenerator,
    api::{AppState, invocation_next, invocation_response, rambda},
};

#[tokio::main]
async fn main() {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));

    let args = std::env::args().collect::<Vec<_>>();
    let cmd = args.get(1).cloned().unwrap_or("./main".to_string());
    let args = args.get(2..).unwrap_or(&[]).to_vec();

    // runtimeの寿命を10秒に設定
    let mut manager =
        RuntimeManager::new(RuntimeProcessGenerator::new(cmd.to_string(), args), 10000);

    // 最初に一つのプロセスを生成しておく
    manager.init().await.unwrap();
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
        .with_state(AppState::<RuntimeProcessGenerator>::new(manager.clone()));
    let listener = tokio::net::TcpListener::bind("localhost:9001")
        .await
        .unwrap();

    // 1秒ごとに期限切れのプロセスを削除する
    tokio::spawn(async move {
        loop {
            manager.gc().await;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });

    axum::serve(listener, app).await.unwrap();
}
