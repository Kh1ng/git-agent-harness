/**
 * Rust WebSocket server for Git Agent Harness
 * This provides a native WebSocket server that the TypeScript server can use
 * or can be used standalone
 */
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::net::TcpListener;
use std::thread;
use tungstenite::accept;

/// Start the WebSocket server
pub async fn run(host: &str, port: u16) -> Result<()> {
    let addr = format!("{}:{}", host, port);
    let listener =
        TcpListener::bind(&addr).with_context(|| format!("Failed to bind to {}:{}", host, port))?;

    println!("WebSocket server starting on {}:{}", host, port);

    // In a real implementation, we would spawn a async runtime and handle connections
    // For now, this is a basic implementation that will be expanded

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let peer_addr = stream.peer_addr()?;
                println!("New WebSocket connection from {}", peer_addr);

                // Spawn a new thread to handle the connection
                thread::spawn(|| {
                    handle_connection(stream).unwrap_or_else(|e| {
                        eprintln!("Error handling connection: {}", e);
                    });
                });
            }
            Err(e) => {
                eprintln!("Connection failed: {}", e);
            }
        }
    }

    Ok(())
}

/// Handle a WebSocket connection
fn handle_connection(stream: std::net::TcpStream) -> Result<()> {
    let addr = stream.peer_addr()?;
    println!("Handling connection from {}", addr);

    // Perform the WebSocket handshake
    let mut websocket = accept(stream)?;

    // Send a welcome message
    let welcome_message = json!({
        "type": "server.welcome",
        "serverVersion": "0.1.0",
        "serverProviderCatalog": {
            "providers": []
        },
        "sessions": [],
        "providers": {}
    });

    websocket.send(tungstenite::Message::Text(welcome_message.to_string()))?;

    // Handle incoming messages
    loop {
        let msg = websocket.read()?;

        match msg {
            tungstenite::Message::Text(text) => {
                println!("Received: {}", text);

                // Parse the message
                let value: Value = serde_json::from_str(&text)?;

                if let Some(msg_type) = value.get("type").and_then(|v| v.as_str()) {
                    match msg_type {
                        "client.hello" => {
                            // Handle client hello
                            println!("Client connected");
                        }
                        "session.start" => {
                            // Handle session start
                            println!("Session start requested");
                        }
                        "ping" => {
                            // Handle ping
                            let response = json!({
                                "type": "server.ping",
                                "timestamp": chrono::Utc::now().timestamp_millis()
                            });
                            websocket.send(tungstenite::Message::Text(response.to_string()))?;
                        }
                        _ => {
                            println!("Unknown message type: {}", msg_type);
                        }
                    }
                }
            }
            tungstenite::Message::Close(_) => {
                println!("Client disconnected from {}", addr);
                break;
            }
            tungstenite::Message::Ping(_) => {
                // Respond to ping with pong
                websocket.send(tungstenite::Message::Pong(vec![]))?;
            }
            tungstenite::Message::Pong(_) => {
                // Ignore pong
            }
            tungstenite::Message::Binary(_) => {
                // Ignore binary messages for now
            }
            tungstenite::Message::Frame(_) => {
                // Raw frames only surface when reading via the low-level
                // API; accept()'s message-level API never produces them.
            }
        }
    }

    Ok(())
}

/// Start the WebSocket server in a blocking manner (for use from main)
pub fn run_blocking(host: &str, port: u16) -> Result<()> {
    let host = host.to_string();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async { run(&host, port).await })
    });

    Ok(())
}
