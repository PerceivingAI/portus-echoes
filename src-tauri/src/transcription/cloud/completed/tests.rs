use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver};

#[test]
fn completed_file_request_timeout_policy_is_three_hundred_seconds() {
    assert_eq!(COMPLETED_FILE_REQUEST_TIMEOUT, Duration::from_secs(300));
}

#[test]
fn groq_completed_audio_spec_remains_fixed() {
    let spec = spec_for(ProviderId::Groq).unwrap();
    assert_eq!(spec.url, GROQ_URL);
    assert_eq!(spec.response_format, Some("verbose_json"));
    assert!(!spec.stream);
    assert!(matches!(spec.no_speech, NoSpeechRule::GroqSegments));
}

#[test]
fn openai_completed_audio_spec_uses_streaming() {
    let spec = spec_for(ProviderId::Openai).unwrap();
    assert_eq!(spec.url, OPENAI_URL);
    assert!(spec.stream);
    assert_eq!(spec.response_format, None);
    assert!(matches!(spec.no_speech, NoSpeechRule::EmptyText));
}
#[test]
fn cloud_contract_module_contains_no_completed_file_transport_implementation() {
    let contracts = include_str!("../mod.rs");
    for file_transport_symbol in [
        ["req", "west::"].concat(),
        ["multi", "part"].concat(),
        ["COMPLETED_FILE_", "HTTP_CLIENT"].concat(),
        ["MAX_FILE_", "UPLOAD_BYTES"].concat(),
    ] {
        assert!(
            !contracts.contains(&file_transport_symbol),
            "Cloud contract module must not own completed-file transport implementation: {file_transport_symbol}"
        );
    }
}

#[test]
fn completed_transport_contains_no_application_error_routing() {
    let source = include_str!("../completed.rs");
    for forbidden in [
        ["User", "ErrorCode"].concat(),
        ["emit_", "error"].concat(),
        ["transcription", ":error"].concat(),
        ["errorMessage", "ForCode"].concat(),
    ] {
        assert!(
            !source.contains(&forbidden),
            "completed transport must return technical facts and never own application/GUI error routing: {forbidden}"
        );
    }
}

#[test]
fn provider_transport_contains_no_desktop_delivery_or_recording_authority() {
    let sources = [include_str!("../completed.rs"), include_str!("../mod.rs")].concat();
    for forbidden in [
        ["output", "::"].concat(),
        ["Delivery", "Capability"].concat(),
        ["Delivery", "Error"].concat(),
        ["models::", "TranscriptResult"].concat(),
        ["TranscriptResult", " {"].concat(),
        ["run_if_", "authoritative"].concat(),
        ["Send", "Input"].concat(),
        ["xdo", "tool"].concat(),
        ["arboard", "::"].concat(),
    ] {
        assert!(
            !sources.contains(&forbidden),
            "provider transport must terminate above desktop delivery/authority mechanics: {forbidden}"
        );
    }
}

#[test]
fn completed_cloud_transport_remains_multipart_and_live_feed_free() {
    let source = include_str!("../completed.rs");
    assert!(source.contains("reqwest::multipart"));
    assert!(source.contains("MAX_FILE_UPLOAD_BYTES"));
    assert!(source.contains("audio.mp3"));
    assert!(
        !source.contains("CloudLiveFeed"),
        "completed-audio HTTP transport must not gain live capture/feed ownership"
    );
}

fn serve_once(status: u16, body: &str) -> (String, Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let body = body.to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let request = read_request(&mut stream);
        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            403 => "Forbidden",
            413 => "Payload Too Large",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            _ => "Status",
        };
        let response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
        let _ = tx.send(request);
    });
    (
        format!("http://127.0.0.1:{port}/v1/audio/transcriptions"),
        rx,
    )
}

fn serve_held_response(status: u16, body: &str) -> (String, Receiver<Vec<u8>>, mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (request_tx, request_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let body = body.to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        let _ = request_tx.send(request);
        let _ = release_rx.recv();
        let response = format!(
                "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });
    (
        format!("http://127.0.0.1:{port}/v1/audio/transcriptions"),
        request_rx,
        release_tx,
    )
}

fn serve_accept_without_reading() -> (String, Receiver<()>, mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (accepted_tx, accepted_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        let _ = accepted_tx.send(());
        let _ = release_rx.recv();
    });
    (
        format!("http://127.0.0.1:{port}/v1/audio/transcriptions"),
        accepted_rx,
        release_tx,
    )
}

fn serve_partial_body() -> (String, Receiver<()>, mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (body_tx, body_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_request(&mut stream);
        let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\nconnection: close\r\n\r\n{\"text\":\"part",
            );
        let _ = stream.flush();
        let _ = body_tx.send(());
        let _ = release_rx.recv();
    });
    (
        format!("http://127.0.0.1:{port}/v1/audio/transcriptions"),
        body_rx,
        release_tx,
    )
}

fn serve_truncated_body() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_request(&mut stream);
        stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\nconnection: close\r\n\r\n{\"text\":\"cut",
                )
                .unwrap();
        stream.flush().unwrap();
    });
    format!("http://127.0.0.1:{port}/v1/audio/transcriptions")
}

fn unavailable_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}/v1/audio/transcriptions")
}

fn read_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        let n = stream.read(&mut tmp).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(headers_end) = find_subslice(&buf, b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buf[..headers_end]).to_string();
            let len = content_length(&headers).unwrap_or(0);
            if buf.len() >= headers_end + 4 + len {
                break;
            }
        }
    }
    buf
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn content_length(headers: &str) -> Option<usize> {
    headers
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("content-length:"))
        .and_then(|line| line.split(':').nth(1)?.trim().parse().ok())
}

fn tiny_mp3() -> Vec<u8> {
    crate::audio::convert::to_mp3_bytes(&[0.0; 1600], 16000).unwrap()
}
fn tiny_wav() -> Vec<u8> {
    tiny_mp3()
}

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn request_text(request: &[u8]) -> String {
    String::from_utf8_lossy(request).to_lowercase()
}

fn run_async<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Runtime::new().unwrap().block_on(future)
}

fn transcribe_with(
    spec: &ProviderSpec,
    url: &str,
    api_key: &str,
    model: &str,
    wav: &[u8],
    language: Option<&str>,
) -> Result<Option<String>, CompletedTransportError> {
    let cancellation = CancellationToken::new();
    match run_async(super::transcribe_with(
        ProviderId::Groq,
        spec,
        url,
        api_key,
        model,
        wav.to_vec(),
        language,
        &cancellation,
        None,
    ))? {
        CloudTranscriptionOutcome::Transcript(text)
        | CloudTranscriptionOutcome::StreamedLive(text) => Ok(Some(text)),
        CloudTranscriptionOutcome::NoSpeech => Ok(None),
        CloudTranscriptionOutcome::Cancelled => {
            panic!("uncancelled transport test unexpectedly cancelled")
        }
    }
}

fn transcribe_completed_file_typed(
    provider: ProviderId,
    api_key: &str,
    model: &str,
    wav: &[u8],
    language: Option<&str>,
) -> Result<Option<String>, CompletedTransportError> {
    let cancellation = CancellationToken::new();
    match run_async(super::transcribe_completed_file(
        provider,
        api_key,
        model,
        wav.to_vec(),
        language,
        &cancellation,
        None,
    ))? {
        CloudTranscriptionOutcome::Transcript(text)
        | CloudTranscriptionOutcome::StreamedLive(text) => Ok(Some(text)),
        CloudTranscriptionOutcome::NoSpeech => Ok(None),
        CloudTranscriptionOutcome::Cancelled => {
            panic!("uncancelled transport test unexpectedly cancelled")
        }
    }
}
#[test]
fn oversized_payload_is_typed_and_rejected_before_network() {
    let spec = spec_for(ProviderId::Openai).unwrap();
    let wav = vec![0u8; MAX_FILE_UPLOAD_BYTES + 1];
    assert_eq!(
        transcribe_with(
            &spec,
            "http://127.0.0.1:1/v1/audio/transcriptions",
            "key",
            "model",
            &wav,
            None,
        ),
        Err(CompletedTransportError::PayloadTooLarge)
    );
}

#[test]
fn request_construction_failure_is_typed() {
    assert!(matches!(
        build_file_part(tiny_wav(), "invalid\nmime"),
        Err(CompletedTransportError::RequestBuildFailed)
    ));
}

#[test]
fn transport_failure_is_typed() {
    let _lock = TEST_LOCK.lock();
    close_completed_file_client();
    let spec = spec_for(ProviderId::Openai).unwrap();
    let err =
    transcribe_with(&spec, &unavailable_url(), "key", "model", &tiny_mp3(), None).unwrap_err();
    assert_eq!(err, CompletedTransportError::TransportFailed);
}

#[test]
fn openai_request_shape_and_json_parse() {
    let _lock = TEST_LOCK.lock();
    let (url, rx) = serve_once(200, r#"{"text":"  hello world  "}"#);
    let spec = spec_for(ProviderId::Openai).unwrap();
    let transcript = transcribe_with(
        &spec,
        &url,
        "test-key",
        "environment-openai-model",
        &tiny_mp3(),
        None,
    )
    .unwrap()
    .expect("successful response must contain a transcript");
    assert_eq!(transcript, "hello world");

    let request = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let request_text = request_text(&request);
    assert!(request_text.starts_with("post /v1/audio/transcriptions http/1.1"));
    assert!(request_text.contains("authorization: bearer test-key"));
    assert!(request_text.contains("multipart/form-data; boundary="));
    assert!(
        request_text.contains("name=\"model\"")
            && request_text.contains("environment-openai-model")
    );
    assert!(request_text.contains("name=\"stream\"") && request_text.contains("true"));
    assert!(!request_text.contains("verbose_json"));
    assert!(request_text.contains("name=\"file\"; filename=\"audio.mp3\""));
    assert!(request_text.contains("audio/mpeg"));
    assert!(!request_text.contains("name=\"language\""));
    assert!(find_subslice(&request, b"ID3").is_some() || request.iter().any(|&b| b == 0xFF));
}

#[test]
fn groq_request_shape_verbose_json_and_language() {
    let _lock = TEST_LOCK.lock();
    let (url, rx) = serve_once(
        200,
        r#"{"text":"hello world","segments":[{"no_speech_prob":0.01,"text":"hello world"}]}"#,
    );
    let spec = spec_for(ProviderId::Groq).unwrap();
    let text = transcribe_with(
        &spec,
        &url,
        "groq-key",
        "environment-groq-model",
        &tiny_mp3(),
        Some("en"),
    )
    .unwrap()
    .expect("successful response must contain a transcript");
    assert_eq!(text, "hello world");
    let request = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let text = request_text(&request);
    assert!(text.contains("authorization: bearer groq-key"));
    assert!(text.contains("verbose_json"));
    assert!(text.contains("environment-groq-model"));
    assert!(text.contains("name=\"language\"") && text.contains("\r\n\r\nen\r\n"));
}

#[test]
fn openai_empty_text_produces_no_transcript() {
    let _lock = TEST_LOCK.lock();
    let (url, _rx) = serve_once(200, r#"{"text":"   "}"#);
    let spec = spec_for(ProviderId::Openai).unwrap();
    assert_eq!(
        transcribe_with(&spec, &url, "k", "m", &tiny_mp3(), None).unwrap(),
        None
    );
}

#[test]
fn groq_all_segments_high_no_speech_prob_produce_no_transcript() {
    let _lock = TEST_LOCK.lock();
    let (url, _rx) = serve_once(
        200,
        r#"{"text":"you","segments":[{"no_speech_prob":0.9},{"no_speech_prob":0.7}]}"#,
    );
    let spec = spec_for(ProviderId::Groq).unwrap();
    assert_eq!(
        transcribe_with(&spec, &url, "k", "m", &tiny_mp3(), None).unwrap(),
        None
    );
}

#[test]
fn groq_mixed_segments_are_speech() {
    let _lock = TEST_LOCK.lock();
    let (url, _rx) = serve_once(
        200,
        r#"{"text":"real words","segments":[{"no_speech_prob":0.9},{"no_speech_prob":0.1}]}"#,
    );
    let spec = spec_for(ProviderId::Groq).unwrap();
    let text = transcribe_with(&spec, &url, "k", "m", &tiny_wav(), None)
        .unwrap()
        .expect("mixed speech segments must produce a transcript");
    assert_eq!(text, "real words");
}

#[test]
fn preserves_401_status() {
    assert_status(401);
}

#[test]
fn non_success_body_text_has_zero_effect_on_transport_fact() {
    let _lock = TEST_LOCK.lock();
    let (url, _rx) = serve_once(
        401,
        r#"{"error":{"message":"429 rate limit network failure use another provider"}}"#,
    );
    let spec = spec_for(ProviderId::Openai).unwrap();
    let err = transcribe_with(&spec, &url, "k", "m", &tiny_wav(), None).unwrap_err();
    assert_eq!(err, CompletedTransportError::HttpStatus(401));
}

#[test]
fn preserves_403_status() {
    assert_status(403);
}

#[test]
fn preserves_413_status() {
    assert_status(413);
}

#[test]
fn preserves_429_status() {
    assert_status(429);
}

#[test]
fn preserves_other_http_status() {
    assert_status(500);
}

fn assert_status(status: u16) {
    let _lock = TEST_LOCK.lock();
    let (url, _rx) = serve_once(status, r#"{"error":{"message":"ignored"}}"#);
    let spec = spec_for(ProviderId::Openai).unwrap();
    let err = transcribe_with(&spec, &url, "k", "m", &tiny_wav(), None).unwrap_err();
    assert_eq!(err, CompletedTransportError::HttpStatus(status));
}

#[test]
fn truncated_success_body_is_response_read_failure() {
    let _lock = TEST_LOCK.lock();
    let spec = spec_for(ProviderId::Openai).unwrap();
    let err =
        transcribe_with(&spec, &serve_truncated_body(), "k", "m", &tiny_wav(), None).unwrap_err();
    assert_eq!(err, CompletedTransportError::ResponseReadFailed);
}

#[test]
fn malformed_success_json_is_response_invalid() {
    let _lock = TEST_LOCK.lock();
    let (url, _rx) = serve_once(200, "this is not json");
    let spec = spec_for(ProviderId::Openai).unwrap();
    let err = transcribe_with(&spec, &url, "k", "m", &tiny_wav(), None).unwrap_err();
    assert_eq!(err, CompletedTransportError::ResponseInvalid);
}

#[test]
fn local_provider_is_structurally_rejected() {
    assert_eq!(
        transcribe_completed_file_typed(ProviderId::Local, "k", "m", &tiny_wav(), None),
        Err(CompletedTransportError::UnsupportedProvider)
    );
}
#[test]
fn held_request_does_not_serialize_a_new_request() {
    let _lock = TEST_LOCK.lock();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (url_a, request_a_rx, release_a) = serve_held_response(200, r#"{"text":"first"}"#);
    let (url_b, request_b_rx) = serve_once(200, r#"{"text":"second"}"#);

    let task_a = runtime.spawn(async move {
        let spec = spec_for(ProviderId::Openai).unwrap();
        let cancellation = CancellationToken::new();
        super::transcribe_with(
            ProviderId::Openai,
            &spec,
            &url_a,
            "key-a",
            "gpt-transcribe",
            tiny_wav(),
            None,
            &cancellation,
            None,
        )
        .await
    });
    request_a_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let task_b = runtime.spawn(async move {
        let spec = spec_for(ProviderId::Groq).unwrap();
        let cancellation = CancellationToken::new();
        super::transcribe_with(
            ProviderId::Groq,
            &spec,
            &url_b,
            "key-b",
            "model-b",
            tiny_wav(),
            None,
            &cancellation,
            None,
        )
        .await
    });

    request_b_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("B must reach its server while A is still held");
    assert_eq!(
        runtime.block_on(task_b).unwrap(),
        Ok(CloudTranscriptionOutcome::Transcript("second".to_string()))
    );

    release_a.send(()).unwrap();
    assert_eq!(
        runtime.block_on(task_a).unwrap(),
        Ok(CloudTranscriptionOutcome::Transcript("first".to_string()))
    );
}

#[test]
fn newest_job_can_finish_and_commit_delivery_while_old_request_is_held() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let _lock = TEST_LOCK.lock();
    close_completed_file_client();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (url_a, request_a_rx, release_a) = serve_held_response(200, r#"{"text":"first"}"#);
    let (url_b, request_b_rx) = serve_once(200, r#"{"text":"second"}"#);
    let authoritative = Arc::new(AtomicU64::new(1));
    let (delivery_tx, delivery_rx) = mpsc::channel();

    let authority_a = Arc::clone(&authoritative);
    let delivery_a = delivery_tx.clone();
    let task_a = runtime.spawn(async move {
        let spec = spec_for(ProviderId::Openai).unwrap();
        let cancellation = CancellationToken::new();
        let outcome = super::transcribe_with(
            ProviderId::Openai,
            &spec,
            &url_a,
            "key-a",
            "gpt-transcribe",
            tiny_wav(),
            None,
            &cancellation,
            None,
        )
        .await;
        if matches!(outcome, Ok(CloudTranscriptionOutcome::Transcript(_) | CloudTranscriptionOutcome::StreamedLive(_)))
            && authority_a.load(Ordering::SeqCst) == 1
        {
            delivery_a.send(1u64).unwrap();
        }
        outcome
    });
    request_a_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    authoritative.store(2, Ordering::SeqCst);
    let authority_b = Arc::clone(&authoritative);
    let delivery_b = delivery_tx.clone();
    let task_b = runtime.spawn(async move {
        let spec = spec_for(ProviderId::Groq).unwrap();
        let cancellation = CancellationToken::new();
        let outcome = super::transcribe_with(
            ProviderId::Groq,
            &spec,
            &url_b,
            "key-b",
            "model-b",
            tiny_wav(),
            None,
            &cancellation,
            None,
        )
        .await;
        if matches!(outcome, Ok(CloudTranscriptionOutcome::Transcript(_) | CloudTranscriptionOutcome::StreamedLive(_)))
            && authority_b.load(Ordering::SeqCst) == 2
        {
            delivery_b.send(2u64).unwrap();
        }
        outcome
    });

    request_b_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("new job must reach its server while old request remains held");
    assert_eq!(
        runtime.block_on(task_b).unwrap(),
        Ok(CloudTranscriptionOutcome::Transcript("second".to_string()))
    );
    assert_eq!(
        delivery_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        2,
        "new authoritative job must commit delivery before old request finishes"
    );

    release_a.send(()).unwrap();
    assert_eq!(
        runtime.block_on(task_a).unwrap(),
        Ok(CloudTranscriptionOutcome::Transcript("first".to_string()))
    );
    assert!(
        delivery_rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "superseded old result must not commit a visible delivery effect"
    );
}
#[test]
fn cancellation_during_upload_send_is_internal() {
    let _lock = TEST_LOCK.lock();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (url, accepted_rx, release_server) = serve_accept_without_reading();
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task = runtime.spawn(async move {
        let spec = spec_for(ProviderId::Openai).unwrap();
        let wav = vec![0u8; 24 * 1024 * 1024];
        super::transcribe_with(
            ProviderId::Openai,
            &spec,
            &url,
            "key",
            "gpt-transcribe",
            wav,
            None,
            &task_cancellation,
            None,
        )
        .await
    });

    accepted_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    cancellation.cancel();
    assert_eq!(
        runtime.block_on(task).unwrap(),
        Ok(CloudTranscriptionOutcome::Cancelled)
    );
    let _ = release_server.send(());
}

#[test]
fn cancellation_while_waiting_for_response_is_internal() {
    let _lock = TEST_LOCK.lock();
    close_completed_file_client();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (url, request_rx, release_server) = serve_held_response(200, r#"{"text":"late"}"#);
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task = runtime.spawn(async move {
        let spec = spec_for(ProviderId::Openai).unwrap();
        super::transcribe_with(
            ProviderId::Openai,
            &spec,
            &url,
            "key",
            "gpt-transcribe",
            tiny_wav(),
            None,
            &task_cancellation,
            None,
        )
        .await
    });

    request_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    cancellation.cancel();
    assert_eq!(
        runtime.block_on(task).unwrap(),
        Ok(CloudTranscriptionOutcome::Cancelled)
    );
    let _ = release_server.send(());
}

#[test]
fn cancellation_during_response_body_read_is_internal() {
    let _lock = TEST_LOCK.lock();
    close_completed_file_client();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (url, body_rx, release_server) = serve_partial_body();
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task = runtime.spawn(async move {
        let spec = spec_for(ProviderId::Openai).unwrap();
        super::transcribe_with(
            ProviderId::Openai,
            &spec,
            &url,
            "key",
            "gpt-transcribe",
            tiny_wav(),
            None,
            &task_cancellation,
            None,
        )
        .await
    });

    body_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    cancellation.cancel();
    assert_eq!(
        runtime.block_on(task).unwrap(),
        Ok(CloudTranscriptionOutcome::Cancelled)
    );
    let _ = release_server.send(());
}

#[test]
fn closing_cache_does_not_abort_an_in_flight_client_clone() {
    let _lock = TEST_LOCK.lock();
    close_completed_file_client();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (url, request_rx, release_server) = serve_held_response(200, r#"{"text":"survives"}"#);
    let task = runtime.spawn(async move {
        let spec = spec_for(ProviderId::Openai).unwrap();
        let cancellation = CancellationToken::new();
        super::transcribe_with(
            ProviderId::Openai,
            &spec,
            &url,
            "key",
            "gpt-transcribe",
            tiny_wav(),
            None,
            &cancellation,
            None,
        )
        .await
    });

    request_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(COMPLETED_FILE_HTTP_CLIENT.lock().is_some());
    close_completed_file_client();
    assert!(COMPLETED_FILE_HTTP_CLIENT.lock().is_none());
    release_server.send(()).unwrap();
    assert_eq!(
        runtime.block_on(task).unwrap(),
        Ok(CloudTranscriptionOutcome::Transcript(
            "survives".to_string()
        ))
    );
    assert!(COMPLETED_FILE_HTTP_CLIENT.lock().is_none());
}
#[test]
fn close_completed_file_client_resets_static_client() {
    let _lock = TEST_LOCK.lock();
    close_completed_file_client();
    assert!(COMPLETED_FILE_HTTP_CLIENT.lock().is_none());
    let (url, _rx) = serve_once(200, r#"{"text":"hello"}"#);
    let spec = spec_for(ProviderId::Openai).unwrap();
    let _ = transcribe_with(&spec, &url, "k", "m", &tiny_wav(), None).unwrap();
    assert!(COMPLETED_FILE_HTTP_CLIENT.lock().is_some());

    close_completed_file_client();
    assert!(COMPLETED_FILE_HTTP_CLIENT.lock().is_none());
}

#[test]
fn openai_sse_streaming_delivers_deltas_incrementally() {
    let _lock = TEST_LOCK.lock();
    close_completed_file_client();
    let sse_body = "data: {\"type\":\"transcript.text.delta\",\"text\":\"Hello\"}\n\ndata: {\"type\":\"transcript.text.delta\",\"text\":\" world\"}\n\ndata: {\"type\":\"transcript.text.done\",\"text\":\"Hello world\"}\n\n";
    let (url, request_rx) = serve_once(200, sse_body);
    let spec = spec_for(ProviderId::Openai).unwrap();
    let cancellation = CancellationToken::new();
    let mut collected = Vec::new();
    let mut on_delta = |delta: String| {
        collected.push(delta);
        Ok(())
    };

    let outcome = run_async(super::transcribe_with(
        ProviderId::Openai,
        &spec,
        &url,
        "key",
        "gpt-transcribe",
        tiny_wav(),
        None,
        &cancellation,
        Some(&mut on_delta),
    ))
    .unwrap();

    let request = request_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let request_str = String::from_utf8_lossy(&request);
    assert!(request_str.contains("stream"));
    assert_eq!(collected, vec!["Hello".to_string(), " world".to_string()]);
    assert_eq!(
        outcome,
        CloudTranscriptionOutcome::StreamedLive("Hello world".to_string())
    );
}

#[test]
fn openai_gpt4o_transcribe_sse_streaming_delivers_delta_field_incrementally() {
    let _lock = TEST_LOCK.lock();
    close_completed_file_client();
    let sse_body = "data: {\"delta\":\"Testing \"}\n\ndata: {\"delta\":\"incremental \"}\n\ndata: {\"delta\":\"streaming.\"}\n\ndata: [DONE]\n\n";
    let (url, request_rx) = serve_once(200, sse_body);
    let spec = spec_for(ProviderId::Openai).unwrap();
    let cancellation = CancellationToken::new();
    let mut collected = Vec::new();
    let mut on_delta = |delta: String| {
        collected.push(delta);
        Ok(())
    };

    let outcome = run_async(super::transcribe_with(
        ProviderId::Openai,
        &spec,
        &url,
        "key",
        "gpt-4o-transcribe",
        tiny_wav(),
        None,
        &cancellation,
        Some(&mut on_delta),
    ))
    .unwrap();

    let _ = request_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        collected,
        vec![
            "Testing ".to_string(),
            "incremental ".to_string(),
            "streaming.".to_string(),
        ]
    );
    assert_eq!(
        outcome,
        CloudTranscriptionOutcome::StreamedLive("Testing incremental streaming.".to_string())
    );
}

#[test]
fn openai_sse_streaming_done_with_no_deltas_falls_back_to_done_text() {
    let _lock = TEST_LOCK.lock();
    close_completed_file_client();
    let sse_body = "data: {\"type\":\"transcript.text.done\",\"text\":\"Complete sentence\"}\n\n";
    let (url, _) = serve_once(200, sse_body);
    let spec = spec_for(ProviderId::Openai).unwrap();
    let cancellation = CancellationToken::new();

    let outcome = run_async(super::transcribe_with(
        ProviderId::Openai,
        &spec,
        &url,
        "key",
        "gpt-transcribe",
        tiny_wav(),
        None,
        &cancellation,
        None,
    ))
    .unwrap();

    assert_eq!(
        outcome,
        CloudTranscriptionOutcome::Transcript("Complete sentence".to_string())
    );
}

#[test]
fn openai_sse_streaming_empty_returns_no_speech() {
    let _lock = TEST_LOCK.lock();
    close_completed_file_client();
    let sse_body = "data: {\"type\":\"transcript.text.done\",\"text\":\"\"}\n\n";
    let (url, _) = serve_once(200, sse_body);
    let spec = spec_for(ProviderId::Openai).unwrap();
    let cancellation = CancellationToken::new();

    let outcome = run_async(super::transcribe_with(
        ProviderId::Openai,
        &spec,
        &url,
        "key",
        "gpt-transcribe",
        tiny_wav(),
        None,
        &cancellation,
        None,
    ))
    .unwrap();

    assert_eq!(outcome, CloudTranscriptionOutcome::NoSpeech);
}

#[test]
fn openai_gpt_transcribe_encodes_plural_languages_field() {
    let _lock = TEST_LOCK.lock();
    close_completed_file_client();
    let (url, request_rx) = serve_once(200, "data: {\"type\":\"transcript.text.done\",\"text\":\"hola\"}\n\n");
    let spec = spec_for(ProviderId::Openai).unwrap();
    let cancellation = CancellationToken::new();

    let _ = run_async(super::transcribe_with(
        ProviderId::Openai,
        &spec,
        &url,
        "key",
        "gpt-transcribe",
        tiny_wav(),
        Some("es"),
        &cancellation,
        None,
    ))
    .unwrap();

    let request = request_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let request_str = String::from_utf8_lossy(&request);
    assert!(request_str.contains("languages[]"));
    assert!(request_str.contains("es"));
}

#[test]
#[ignore = "writes to the OS credential store"]
fn store_bogus_openai_key() {
    crate::keystore::platform()
        .set_key_typed(ProviderId::Openai, "sk-bogus-live-401-check")
        .unwrap();
}

#[test]
#[ignore = "writes to the OS credential store"]
fn clear_bogus_openai_key() {
    crate::keystore::platform()
        .delete_key_typed(ProviderId::Openai)
        .unwrap();
}
