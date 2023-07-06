use crate::errors::*;

static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

pub struct Client {
    http: reqwest::Client,
}

impl Client {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(APP_USER_AGENT)
            .build()?;
        Ok(Client { http })
    }

    pub async fn fetch(&self, url: &str) -> Result<bytes::Bytes> {
        info!("Downloading {url:?}...");
        let response = self
            .http
            .get(url)
            .send()
            .await
            .context("Failed to send http request")?
            .error_for_status()
            .context("Received http error")?;
        let buf = response.bytes().await.context("Failed to read http body")?;
        Ok(buf)
    }
}
