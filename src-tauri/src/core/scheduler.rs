use std::future::Future;
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct SchedulerConfig {
    pub interval: Duration,
}

pub struct Scheduler {
    config: SchedulerConfig,
}

impl Scheduler {
    pub fn new(config: SchedulerConfig) -> Self {
        Self { config }
    }

    pub async fn run<F, Fut>(&self, mut refresh: F)
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send,
    {
        loop {
            if let Err(err) = refresh().await {
                eprintln!("usage refresh failed: {err:#}");
            }
            tokio::time::sleep(self.config.interval).await;
        }
    }
}
