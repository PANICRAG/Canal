//! Remote agent client for microservice mode.
//!
//! Forwards chat requests to agent-service via gRPC streaming,
//! converting AgentEvent → StreamEvent (SSE format the frontend expects).

use axum::response::sse::Event;
use gateway_core::chat::streaming::StreamEvent;
use std::convert::Infallible;
use tokio::sync::mpsc;
use tonic::transport::Channel;
use uuid::Uuid;

use canal_proto::agent::{
    agent_event, agent_service_client::AgentServiceClient, AgentChatRequest,
};

/// Client wrapper for the remote agent-service.
#[derive(Clone)]
pub struct RemoteAgentClient {
    client: AgentServiceClient<Channel>,
}

impl RemoteAgentClient {
    /// Connect to the agent-service at the given URL.
    pub async fn connect(url: String) -> Result<Self, tonic::transport::Error> {
        let client = AgentServiceClient::connect(url).await?;
        Ok(Self { client })
    }

    /// Stream a chat request through the remote agent-service.
    ///
    /// Returns a channel receiver yielding SSE Events in the same format
    /// as the monolith path — the frontend sees no difference.
    pub async fn chat_stream(
        &self,
        session_id: Uuid,
        message: String,
        model: Option<String>,
    ) -> Result<mpsc::Receiver<Result<Event, Infallible>>, tonic::Status> {
        let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(128);

        let request = AgentChatRequest {
            session_id: session_id.to_string(),
            message,
            model,
            collaboration_mode: None,
            trace: None,
        };

        let mut client = self.client.clone();
        let mut stream = client.chat(request).await?.into_inner();

        let message_id = Uuid::new_v4();

        // Send start event
        let start_event = StreamEvent::Start {
            conversation_id: session_id,
            message_id,
            session_id: Some(session_id.to_string()),
        };
        let _ = send_sse(&tx, &start_event).await;

        // Spawn task to forward gRPC events → SSE events
        tokio::spawn(async move {
            while let Ok(Some(agent_event)) = stream.message().await {
                match agent_event.event {
                    Some(agent_event::Event::Text(chunk)) => {
                        let event = StreamEvent::Text { chunk: chunk.text };
                        if send_sse(&tx, &event).await.is_err() {
                            break;
                        }
                    }
                    Some(agent_event::Event::Thinking(chunk)) => {
                        let event = StreamEvent::Thinking {
                            message: chunk.text,
                        };
                        if send_sse(&tx, &event).await.is_err() {
                            break;
                        }
                    }
                    Some(agent_event::Event::ToolCall(tc)) => {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.arguments_json).unwrap_or_default();
                        let event = StreamEvent::ToolCall {
                            id: tc.tool_id,
                            name: tc.tool_name,
                            arguments: args,
                        };
                        if send_sse(&tx, &event).await.is_err() {
                            break;
                        }
                    }
                    Some(agent_event::Event::ToolResult(tr)) => {
                        let result = serde_json::Value::String(tr.content);
                        let event = StreamEvent::ToolResponse {
                            id: tr.tool_id,
                            name: String::new(),
                            result,
                        };
                        if send_sse(&tx, &event).await.is_err() {
                            break;
                        }
                    }
                    Some(agent_event::Event::Done(done)) => {
                        let event = StreamEvent::Done {
                            message_id,
                            artifacts: vec![],
                            usage: Some(gateway_core::chat::streaming::TokenUsage {
                                prompt_tokens: 0,
                                completion_tokens: 0,
                                total_tokens: done.total_tokens,
                            }),
                        };
                        let _ = send_sse(&tx, &event).await;
                        break;
                    }
                    Some(agent_event::Event::Error(err)) => {
                        let event = StreamEvent::Error {
                            message: err.message,
                            recoverable: false,
                        };
                        let _ = send_sse(&tx, &event).await;
                        break;
                    }
                    None => {}
                }
            }
        });

        Ok(rx)
    }
}

/// Helper to send a StreamEvent as an SSE Event through the channel.
async fn send_sse(
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    stream_event: &StreamEvent,
) -> Result<(), mpsc::error::SendError<Result<Event, Infallible>>> {
    let json = serde_json::to_string(stream_event).unwrap_or_default();
    let event = Event::default().data(json);
    tx.send(Ok(event)).await
}
