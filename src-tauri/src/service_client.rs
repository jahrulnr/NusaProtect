use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use gpapi::{
  service::{
    event::WsEvent,
    request::WsRequest,
    transport::{
      ClientMessage, ClientPrelude, HIP_PROTOCOL_VERSION, NoiseInitiator, ServerMessage, ServerPrelude,
      SessionCredential,
    },
    vpn_state::VpnState,
  },
  utils::endpoint::ws_endpoint,
};
use log::{info, warn};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

pub struct ServiceClient {
  req_tx: mpsc::Sender<ClientMessage>,
}

impl ServiceClient {
  pub async fn send(&self, req: WsRequest) -> anyhow::Result<()> {
    self
      .req_tx
      .send(ClientMessage::Request {
        id: Uuid::new_v4(),
        request: req,
      })
      .await?;
    Ok(())
  }
}

pub async fn connect_to_service(
  credential: Arc<SessionCredential>,
  app_handle: AppHandle,
  client_store: Arc<Mutex<Option<ServiceClient>>>,
) -> anyhow::Result<(ServiceClient, tokio::task::JoinHandle<()>, VpnState)> {
  let endpoint = ws_endpoint().await?;

  info!("Connecting to gpservice at {}", endpoint);
  let (mut socket, _) = connect_async(&endpoint).await?;
  info!("WebSocket connection established");

  // 1. ClientPrelude (plain JSON in a binary frame)
  let prelude = ClientPrelude {
    product_version: credential.product_version().to_owned(),
    hip_protocol_version: HIP_PROTOCOL_VERSION,
    service_instance_id: credential.service_instance_id(),
    session_id: credential.session_id(),
  };
  socket.send(Message::Binary(prelude.encode()?.into())).await?;

  // 2. ServerPrelude
  let server_prelude = ServerPrelude::decode(&recv_binary(&mut socket).await?)
    .map_err(|e| anyhow::anyhow!("Failed to decode server prelude: {}", e))?;
  match server_prelude {
    ServerPrelude::Ready => info!("Service accepted handshake"),
    ServerPrelude::Rejected(rejection) => {
      anyhow::bail!("Service rejected handshake: {:?}", rejection)
    }
  }

  // 3. Noise handshake (psk = credential secret)
  let mut initiator = NoiseInitiator::new(
    credential.product_version(),
    credential.service_instance_id(),
    credential.session_id(),
    credential.secret(),
  )?;
  socket
    .send(Message::Binary(initiator.write_first()?.into()))
    .await?;
  let noise_response = recv_binary(&mut socket).await?;
  let mut noise = initiator
    .read_response(&noise_response)
    .map_err(|e| anyhow::anyhow!("Noise handshake failed: {}", e))?;
  info!("Noise transport established");

  // 4. Attach (first encrypted message must be Attach)
  // A future reattach/resume implementation could send the highest event
  // revision seen so far instead of None.
  let attach = noise
    .encrypt(&ClientMessage::Attach {
      last_event_revision: None,
    })
    .map_err(|e| anyhow::anyhow!("Failed to encrypt attach: {}", e))?;
  socket.send(Message::Binary(attach.into())).await?;

  // 5. Attached confirmation + initial snapshot. The snapshot's VpnState is
  // returned so the caller can bundle it into the "service-connected" event —
  // the "vpn-state" emit here can race past frontend listener registration.
  let attached = recv_binary(&mut socket).await?;
  let vpn_state = match noise
    .decrypt::<ServerMessage>(&attached)
    .map_err(|e| anyhow::anyhow!("Failed to decrypt attach response: {}", e))?
  {
    ServerMessage::Attached {
      connection_id,
      current_revision,
      snapshot,
    } => {
      info!("Attached to gpservice as {} (revision {})", connection_id, current_revision);
      let _ = app_handle.emit("vpn-state", &snapshot.vpn_state);
      let _ = app_handle.emit("vpn-env", &snapshot);
      snapshot.vpn_state
    }
    other => anyhow::bail!("Expected Attached message, got {:?}", other),
  };

  let (req_tx, mut req_rx) = mpsc::channel::<ClientMessage>(32);

  // Outgoing requests + incoming messages share this task so the
  // NoiseTransport (single-direction cipher state) stays serialized.
  let io_done = tokio::spawn(async move {
    loop {
      tokio::select! {
        req = req_rx.recv() => {
          let Some(req) = req else { break };
          match noise.encrypt(&req) {
            Ok(data) => {
              if let Err(e) = socket.send(Message::Binary(data.into())).await {
                warn!("Failed to send WS message: {}", e);
                break;
              }
            }
            Err(e) => warn!("Failed to encrypt request: {}", e),
          }
        }
        msg = socket.next() => {
          match msg {
            Some(Ok(Message::Binary(data))) => match noise.decrypt::<ServerMessage>(&data) {
              Ok(message) => handle_server_message(message, &app_handle),
              Err(e) => warn!("Failed to decrypt service message: {}", e),
            },
            Some(Ok(Message::Ping(_))) => {
              // tungstenite queues the pong automatically on the next flush
            }
            Some(Ok(Message::Close(_))) | None => {
              info!("gpservice closed the connection");
              break;
            }
            Some(Err(e)) => {
              warn!("WebSocket error: {}", e);
              break;
            }
            _ => {}
          }
        }
      }
    }
    *client_store.lock().await = None;
    let _ = app_handle.emit("service-disconnected", ());
  });

  Ok((ServiceClient { req_tx }, io_done, vpn_state))
}

async fn recv_binary<S>(socket: &mut S) -> anyhow::Result<Vec<u8>>
where
  S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
  match socket.next().await {
    Some(Ok(Message::Binary(data))) => Ok(data.to_vec()),
    Some(Ok(_)) => anyhow::bail!("Expected binary frame during handshake"),
    Some(Err(e)) => anyhow::bail!("WebSocket error during handshake: {}", e),
    None => anyhow::bail!("Connection closed during handshake"),
  }
}

fn handle_server_message(message: ServerMessage, app_handle: &AppHandle) {
  match message {
    ServerMessage::Event { event, .. } => handle_event(event, app_handle),
    ServerMessage::Snapshot { snapshot, .. } => {
      let _ = app_handle.emit("vpn-state", &snapshot.vpn_state);
      let _ = app_handle.emit("vpn-env", &snapshot);
    }
    ServerMessage::Reply { id, result } => {
      use gpapi::service::transport::ServiceResult;
      match result {
        ServiceResult::Accepted => info!("Request {} accepted", id),
        ServiceResult::Rejected(rejection) => {
          warn!("Request {} rejected: {:?}", id, rejection);
          let _ = app_handle.emit("request-rejected", rejection.message());
        }
        other => info!("Request {} reply: {:?}", id, other),
      }
    }
    ServerMessage::Attached { .. } | ServerMessage::TunnelStatistics { .. } => {}
  }
}

fn handle_event(event: WsEvent, app_handle: &AppHandle) {
  match event {
    WsEvent::VpnState(state) => {
      info!("VPN state changed: {:?}", state);
      let _ = app_handle.emit("vpn-state", &state);
    }
    WsEvent::VpnEnv(env) => {
      info!("Received VPN environment from service");
      // Emit both the env (for initial state + auth path) and the embedded state
      let _ = app_handle.emit("vpn-state", &env.vpn_state);
      let _ = app_handle.emit("vpn-env", &env);
    }
    WsEvent::TunnelState(state) => {
      let _ = app_handle.emit("tunnel-state", &state);
    }
    WsEvent::ActiveGui => {
      info!("Another GUI instance became active");
      let _ = app_handle.emit("active-gui", ());
    }
    WsEvent::ResumeConnection => {
      info!("Resume connection requested");
      let _ = app_handle.emit("resume-connection", ());
    }
  }
}
