use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};

use futures_util::{
    sink::SinkExt,
    stream::{SplitStream, StreamExt},
};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

struct AppState {
    // channel used to send messages to all connected clients
    tx: broadcast::Sender<String>,
}

impl Default for AppState {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(16);
        Self { tx }
    }
}

async fn handle_incoming_message(
    mut stream: SplitStream<WebSocket>,
    tx_chat: broadcast::Sender<String>,
    sender: mpsc::Sender<String>,
) {
    while let Some(Ok(Message::Text(text))) = stream.next().await {
        let _ = tx_chat.send(text);
        if sender
            .send(String::from("Your message has been sent"))
            .await
            .is_err()
        {
            break;
        }
    }
}

async fn forward_chat_messages(
    mut rx_chat: broadcast::Receiver<String>,
    sender: mpsc::Sender<String>,
) {
    while let Ok(msg) = rx_chat.recv().await {
        if sender.send(format!("New message: {}", msg)).await.is_err() {
            break;
        }
    }
}

async fn websocket(stream: WebSocket, state: Arc<AppState>) {
    // split the websocket stream into a sender (sink) and receiver (stream)
    let (mut sink, mut stream) = stream.split();
    // create an mpsc so we can send messages to the sink from multiple threads
    let (sender, mut receiver) = mpsc::channel::<String>(16);

    // spawn a task that forwards messages from the mpsc to the sink
    tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            if sink.send(message.into()).await.is_err() {
                break;
            }
        }
    });

    // subscribe to the chat channel
    let mut rx_chat = state.tx.subscribe();

    // whenever a chat is sent to rx_chat, forward it to the mpsc
    let send_task_sender = sender.clone();
    let mut send_task = tokio::spawn(forward_chat_messages(rx_chat, send_task_sender));

    // clone the tx channel so we can send messages to it
    let tx_chat = state.tx.clone();

    // whenever a user sends a chat, send it to the tx_chat
    let recv_task_sender = sender.clone();
    let mut recv_task = tokio::spawn(handle_incoming_message(stream, tx_chat, recv_task_sender));

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| websocket(socket, state))
}

#[tokio::main]
async fn main() {
    // Set up application state for use with with_state().
    let app = Router::new()
        .route("/ws", get(websocket_handler))
        .with_state(Arc::new(AppState::default()));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
