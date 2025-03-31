use crate::errors::*;
use std::time::Duration;

static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
pub const READ_TIMEOUT: Duration = Duration::from_secs(240);

pub struct Client {
    http: reqwest::Client,
}

impl Client {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(APP_USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()?;
        Ok(Client { http })
    }

    pub async fn request(&self, url: &str) -> Result<reqwest::Response> {
        info!("Downloading {url:?}...");
        let response = self
            .http
            .get(url)
            .send()
            .await
            .context("Failed to send http request")?
            .error_for_status()
            .context("Received http error")?;
        Ok(response)
    }

    pub async fn fetch(&self, url: &str) -> Result<bytes::Bytes> {
        let response = self.request(url).await?;
        let buf = response.bytes().await.context("Failed to read http body")?;
        Ok(buf)
    }
}
