use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use url::Url;
use gomoku_core::protocol::ClientCommand;

pub const RECORD_SEPARATOR: char = '\u{001e}';

#[derive(Clone, Debug)]
pub struct SignalRClient {
    inner: Arc<Mutex<SignalRState>>,
    debug_websocket: bool,
}

#[derive(Debug)]
struct SignalRState {
    sender: futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, Message>,
    next_invocation_id: u64,
    pending: HashMap<String, oneshot::Sender<Result<Value>>>,
    last_server_error: Option<String>,
    handshake_tx: Option<oneshot::Sender<()>>,
}

#[derive(Clone, Debug)]
pub struct HubEvent {
    pub target: String,
    pub arguments: Vec<Value>,
}

impl SignalRClient {
    pub async fn connect(
        url: &Url,
        debug_websocket: bool,
    ) -> Result<(Self, mpsc::UnboundedReceiver<HubEvent>)> {
        let (websocket, _) = connect_async(url.as_str())
            .await
            .with_context(|| format!("failed to connect to {}", url))?;
        let (sender, mut receiver) = websocket.split();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (handshake_tx, handshake_rx) = oneshot::channel();

        let client = Self {
            inner: Arc::new(Mutex::new(SignalRState {
                sender,
                next_invocation_id: 0,
                pending: HashMap::new(),
                last_server_error: None,
                handshake_tx: Some(handshake_tx),
            })),
            debug_websocket,
        };

        client.send_raw(r#"{"protocol":"json","version":1}"#).await?;

        let cloned = client.clone();
        tokio::spawn(async move {
            while let Some(message) = receiver.next().await {
                let Ok(message) = message else {
                    cloned.fail_pending(None).await;
                    break;
                };

                match message {
                    Message::Text(text) => {
                        cloned.log_ws_receive(&text);
                        if let Err(error) = cloned.handle_text(&text, &event_tx).await {
                            eprintln!("signalr receive error: {error:#}");
                        }
                    }
                    other => {
                        cloned.log_ws_receive(&format!("{other:?}"));
                    }
                }
            }

            cloned.fail_pending(None).await;
        });

        // Wait for the handshake acknowledgement `{}` from the server.
        // It should arrive quickly.
        if tokio::time::timeout(std::time::Duration::from_secs(5), handshake_rx).await.is_err() {
            return Err(anyhow!("timeout waiting for signalr handshake"));
        }

        Ok((client, event_rx))
    }

    

    pub async fn send_void(&self, target: &str, arguments: Vec<Value>) -> Result<()> {
        let envelope = serde_json::json!({
            "type": 1,
            "target": target,
            "arguments": arguments,
        });
        self.send_json(&envelope).await
    }

    pub async fn send_raw(&self, raw: &str) -> Result<()> {
        let mut payload = String::from(raw);
        payload.push(RECORD_SEPARATOR);
        self.log_ws_send(&payload);
        let mut state = self.inner.lock().await;
        state.sender.send(Message::Text(payload.into())).await?;
        Ok(())
    }

    pub async fn send_json(&self, value: &Value) -> Result<()> {
        let raw = serde_json::to_string(value)?;
        self.send_raw(&raw).await
    }

    /// Serialize a typed `ClientCommand` and send it as a SignalR invocation.
    ///
    /// This uses the `ClientCommand` serde shape (tagged `type` / `body`) to
    /// extract the invocation target and the argument body, then sends the
    /// invocation as a fire-and-forget call.
    pub async fn send_command(&self, cmd: &ClientCommand) -> Result<()> {
        let value = serde_json::to_value(cmd)?;
        let target = value
            .get("type")
            .and_then(Value::as_str)
            .context("client command missing type")?;
        let body = value.get("body").cloned().unwrap_or(Value::Null);
        self.send_void(target, vec![body]).await
    }

    /// Invoke a typed `ClientCommand` and wait for a response deserialized into `T`.
    pub async fn invoke_command<T>(&self, cmd: &ClientCommand) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let (target, arguments) = match cmd {
            ClientCommand::SetInitialBoard { room_id, moves_history } => (
                "SetInitialBoard".to_string(),
                vec![Value::String(room_id.clone()), Value::String(moves_history.clone())],
            ),
            ClientCommand::CloseRoom { room_name } => (
                "CloseRoom".to_string(),
                vec![Value::String(room_name.clone())],
            ),
            _ => {
                let value = serde_json::to_value(cmd)?;
                let target = value
                    .get("type")
                    .and_then(Value::as_str)
                    .context("client command missing type")?
                    .to_string();
                let body = value.get("body").cloned().unwrap_or(Value::Null);
                (target, vec![body])
            }
        };

        let invocation_id = {
            let mut state = self.inner.lock().await;
            let invocation_id = state.next_invocation_id.to_string();
            state.next_invocation_id += 1;
            invocation_id
        };

        let (tx, rx) = oneshot::channel();
        {
            let mut state = self.inner.lock().await;
            state.pending.insert(invocation_id.clone(), tx);
        }

        let envelope = serde_json::json!({
            "type": 1,
            "invocationId": invocation_id,
            "target": target,
            "arguments": arguments,
        });
        self.send_json(&envelope).await?;

        let response = rx.await.context("invocation cancelled")??;
        let typed = serde_json::from_value(response).context("failed to decode invoke result")?;
        Ok(typed)
    }

    async fn handle_text(&self, text: &str, event_tx: &mpsc::UnboundedSender<HubEvent>) -> Result<()> {
        for frame in text.split(RECORD_SEPARATOR).filter(|frame| !frame.trim().is_empty()) {
            if frame == "{}" {
                let tx = {
                    let mut state = self.inner.lock().await;
                    state.handshake_tx.take()
                };
                if let Some(tx) = tx {
                    let _ = tx.send(());
                }
                continue;
            }

            let value: Value = serde_json::from_str(frame).context("failed to parse signalr frame")?;
            self.handle_value(value, event_tx).await?;
        }
        Ok(())
    }

            fn log_ws_send(&self, payload: &str) {
                if self.debug_websocket {
                    eprintln!("ws -> {payload}");
                }
            }

            fn log_ws_receive(&self, payload: &str) {
                if self.debug_websocket {
                    eprintln!("ws <- {payload}");
                }
            }

    async fn handle_value(&self, value: Value, event_tx: &mpsc::UnboundedSender<HubEvent>) -> Result<()> {
        if value.get("type").is_none() {
            return Ok(());
        }

        let message_type = value
            .get("type")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("missing signalr message type"))?;

        match message_type {
            1 => {
                let target = value
                    .get("target")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("missing hub target"))?
                    .to_string();
                let arguments = value
                    .get("arguments")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if target == "ConnectionError" {
                    if let Some(message) = arguments.first().and_then(Value::as_str) {
                        let mut state = self.inner.lock().await;
                        state.last_server_error = Some(message.to_string());
                    }
                }
                let _ = event_tx.send(HubEvent { target, arguments });
            }
            3 => {
                let invocation_id = value
                    .get("invocationId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("missing invocation id"))?
                    .to_string();
                let response = if let Some(error) = value.get("error").and_then(Value::as_str) {
                    Err(anyhow!(error.to_string()))
                } else {
                    Ok(value.get("result").cloned().unwrap_or(Value::Null))
                };
                let sender = {
                    let mut state = self.inner.lock().await;
                    state.pending.remove(&invocation_id)
                };
                if let Some(sender) = sender {
                    let _ = sender.send(response);
                }
            }
            6 => {}
            7 => {
                return Ok(());
            }
            _ => {}
        }

        Ok(())
    }

    async fn fail_pending(&self, message: Option<String>) {
        let message = {
            let state = self.inner.lock().await;
            message.or_else(|| state.last_server_error.clone()).unwrap_or_else(|| "server closed the connection".to_string())
        };
        let error = anyhow!(message);
        let mut state = self.inner.lock().await;
        for (_, sender) in state.pending.drain() {
            let _ = sender.send(Err(anyhow!(error.to_string())));
        }
    }
}
