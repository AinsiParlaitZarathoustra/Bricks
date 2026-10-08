//! Wiring: `Agent::builder().reasoning_profile(id)` must reach the wire.
//!
//! The merge itself is unit-tested in `cersei-provider`; this binds the seam
//! between the agent and the provider (an agent that silently dropped the
//! profile would pass every provider test).

use cersei_agent::Agent;
use cersei_provider::ProviderRegistry;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

fn serve_once(bodies: Arc<Mutex<Vec<String>>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        while let Ok((mut sock, _)) = listener.accept() {
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let head_end = loop {
                match sock.read(&mut tmp) {
                    Ok(0) | Err(_) => break None,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(p + 4);
                }
            };
            let Some(head_end) = head_end else { continue };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            while buf.len() < head_end + len {
                match sock.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
            bodies
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[head_end..]).to_string());
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}],",
                "\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
                "data: [DONE]\n\n"
            );
            let payload = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(payload.as_bytes());
            let _ = sock.flush();
            let _ = sock.shutdown(std::net::Shutdown::Write);
        }
    });
    format!("http://127.0.0.1:{port}/v1")
}

fn provider(endpoint: &str) -> cersei_provider::ConfiguredProvider {
    let toml = format!(
        r#"
schema_version = 1
[[providers]]
id = "p"
name = "P"
endpoint = "{endpoint}"
protocol = "chat_completions"
auth = "bearer"
api_key_env = "BRICKS_TEST_API_KEY"
[[providers.models]]
id = "m"
name = "M"
api_model = "am"
streaming = true
tool_calls = true
[providers.models.limits]
max_input_tokens = 100000
max_output_tokens = 4000
[providers.models.reasoning]
default = "careful"
[[providers.models.reasoning.profiles]]
id = "careful"
parameters = {{ effort_knob = "high" }}
[[providers.models.reasoning.profiles]]
id = "quick"
parameters = {{ effort_knob = "low" }}
"#
    );
    ProviderRegistry::from_toml_str(&toml, "t.toml")
        .unwrap()
        .resolve("p/m")
        .unwrap()
        .provider()
        .env(|_| Some("k".into()))
        .build()
        .unwrap()
}

async fn body_for(agent_profile: Option<&str>) -> serde_json::Value {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let url = serve_once(bodies.clone());
    let mut builder = Agent::builder().provider(provider(&url)).max_tokens(32);
    if let Some(id) = agent_profile {
        builder = builder.reasoning_profile(id);
    }
    let agent = builder.build().expect("build agent");
    agent.run("hello").await.expect("run completes");
    let bodies = bodies.lock().unwrap();
    serde_json::from_str(&bodies[0]).expect("request body is JSON")
}

#[tokio::test]
async fn the_agents_profile_reaches_the_request() {
    assert_eq!(body_for(Some("quick")).await["effort_knob"], "low");
}

#[tokio::test]
async fn without_an_agent_profile_the_models_default_applies() {
    let body = body_for(None).await;
    assert_eq!(body["effort_knob"], "high");
    // The profile *id* is never sent as such.
    assert!(!body.to_string().contains("careful"));
    // The label sent as model is the configured api_model, not the agent's label.
    assert_eq!(body["model"], "am");
}
