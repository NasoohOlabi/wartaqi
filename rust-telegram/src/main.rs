use wartaq_telegram::{
    assistant::HelpModel,
    worker::{Config, Worker},
};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("Telegram worker could not start or serve");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if std::env::args().any(|argument| argument == "--assistant-smoke") {
        return if HelpModel::from_env()?.smoke("smoke").await? {
            Ok(())
        } else {
            Err("assistant smoke did not get a usable answer".into())
        };
    }
    let config = Config::from_env()?;
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.port)).await?;
    Worker::new(config)?.run(listener).await
}
