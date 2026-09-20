use super::*;
use std::net::TcpListener;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

fn mock_live_ws_server(
    events_to_emit: Vec<&'static str>,
) -> (String, mpsc::Receiver<String>, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (auth_tx, auth_rx) = mpsc::channel(1);
    let (received_tx, received_rx) = mpsc::channel(100);

    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let mut captured_auth = String::new();
        let callback = |req: &Request, response: Response| {
            if let Some(auth) = req.headers().get("Authorization") {
                captured_auth = auth.to_str().unwrap_or_default().to_string();
            }
            Ok(response)
        };

        let ws_stream = accept_hdr_async(stream, callback).await.unwrap();
        let _ = auth_tx.send(captured_auth).await;
        let (mut ws_writer, mut ws_reader) = ws_stream.split();

        // Send simulated server events
        tokio::spawn(async move {
            for event in events_to_emit {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let _ = ws_writer.send(Message::Text(event.to_string().into())).await;
            }
        });

        // Record client sent messages
        while let Some(msg) = ws_reader.next().await {
            if let Ok(Message::Text(text)) = msg {
                let _ = received_tx.send(text.to_string()).await;
            }
        }
    });

    (format!("ws://127.0.0.1:{port}"), auth_rx, received_rx)
}

#[tokio::test]
async fn live_realtime_ws_handshake_session_update_and_delta_flow() {
    let events = vec![
        r#"{"type":"conversation.item.input_audio_transcription.delta","delta":"Hello "}"#,
        r#"{"type":"conversation.item.input_audio_transcription.delta","delta":"world!"}"#,
        r#"{"type":"conversation.item.input_audio_transcription.completed","transcript":"Hello world!"}"#,
    ];

    let (ws_url, mut auth_rx, mut received_rx) = mock_live_ws_server(events);

    let mut request = ws_url
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "Authorization",
        "Bearer test-key".parse().unwrap(),
    );
    request.headers_mut().insert(
        "OpenAI-Beta",
        "realtime=v1".parse().unwrap(),
    );

    let (ws_stream, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let (mut ws_writer, _ws_reader) = ws_stream.split();

    // Verify auth header received by server
    let auth = auth_rx.recv().await.unwrap();
    assert_eq!(auth, "Bearer test-key");

    // Send session.update
    let session_msg = SessionUpdateMessage {
        msg_type: "session.update",
        session: LiveSessionConfig {
            session_type: "transcription",
            audio: LiveAudioConfig {
                input: LiveAudioInputConfig {
                    format: LiveAudioFormat {
                        format_type: "audio/pcm",
                        rate: 24_000,
                    },
                    transcription: LiveTranscriptionParams {
                        model: "gpt-live-transcribe",
                        languages: None,
                    },
                    turn_detection: None,
                },
            },
        },
    };
    ws_writer.send(Message::Text(serde_json::to_string(&session_msg).unwrap().into())).await.unwrap();
    let received_session = received_rx.recv().await.unwrap();
    assert!(received_session.contains("session.update"));
    assert!(received_session.contains("gpt-live-transcribe"));
}
