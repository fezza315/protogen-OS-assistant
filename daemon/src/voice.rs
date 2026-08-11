//! voice.rs
//! --------
//! Talks to the persistent voice_bridge.py subprocess (spawned by
//! backend.rs) over its stdin/stdout using the newline-delimited JSON
//! protocol documented at the top of voice_bridge.py. This module owns
//! that pipe and exposes async listen()/speak() calls to the rest of the
//! daemon (dispatcher / server on StartListening).
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::Mutex;

#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum BridgeRequest<'a> {
    Listen { seconds: u32 },
    Speak { text: &'a str, voice_bank: Option<&'a str> },
    /// Reserved for a future "switch voice bank" UI action.
    #[allow(dead_code)]
    SetVoiceBank { voice_bank: Option<&'a str> },
    /// Reserved for graceful daemon shutdown (send before killing the
    /// child so voice_bridge.py can release its audio device cleanly).
    #[allow(dead_code)]
    Shutdown,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum BridgeResponse {
    Transcript { text: String },
    Spoke { ok: bool },
    Ack,
    Error { message: String },
}

pub struct VoiceBridge {
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
}

impl VoiceBridge {
    pub fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        Self { stdin: Mutex::new(stdin), stdout: Mutex::new(BufReader::new(stdout)) }
    }

    async fn call(&self, req: BridgeRequest<'_>) -> Result<BridgeResponse> {
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;
        }
        let mut buf = String::new();
        {
            let mut stdout = self.stdout.lock().await;
            stdout.read_line(&mut buf).await?;
        }
        if buf.trim().is_empty() {
            bail!("voice bridge closed the connection");
        }
        Ok(serde_json::from_str(buf.trim())?)
    }

    pub async fn listen(&self, seconds: u32) -> Result<String> {
        match self.call(BridgeRequest::Listen { seconds }).await? {
            BridgeResponse::Transcript { text } => Ok(text),
            BridgeResponse::Error { message } => bail!("voice bridge listen error: {message}"),
            _ => bail!("unexpected response to listen"),
        }
    }

    pub async fn speak(&self, text: &str, voice_bank: Option<&str>) -> Result<bool> {
        match self.call(BridgeRequest::Speak { text, voice_bank }).await? {
            BridgeResponse::Spoke { ok } => Ok(ok),
            BridgeResponse::Error { message } => bail!("voice bridge speak error: {message}"),
            _ => bail!("unexpected response to speak"),
        }
    }
}
