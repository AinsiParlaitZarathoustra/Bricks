//! # Multimodal Input
//!
//! Attach images, audio, or PDFs to a message and send them to a configured
//! model. The same protocol-agnostic [`ContentBlock`]s work everywhere; what a
//! given model accepts is what its configuration declares *and* what its
//! protocol adapter can carry (see the capability matrix in `docs/providers.md`).
//! A block that cannot be carried is refused with an explicit error before
//! anything is sent — it is never silently dropped.
//!
//! ```bash
//! cargo run --example multimodal -- provider_id/model_id path/to/image.png
//! ```
//!
//! High-level entry points shown here:
//!   - `ContentBlock::from_path`     — read a file, auto-detect its media type
//!   - `ContentBlock::image_bytes`   — build from raw bytes you already hold
//!   - `ContentBlock::image_url`     — reference a remote image by URL
//!   - `Message::user_with_files`    — text + several local files in one call

use cersei::prelude::*;
use cersei::provider::{CompletionRequest, Provider, ProviderOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let selection = args.next();
    let paths: Vec<String> = args.collect();
    let Some(selection) = selection.filter(|_| !paths.is_empty()) else {
        eprintln!(
            "usage: multimodal provider_id/model_id <file> [<file> ...]   (image / audio / pdf)"
        );
        std::process::exit(2);
    };

    // ── Build a multimodal user message in one line ─────────────────────────
    // Each file is read from disk, its MIME type is sniffed from the bytes
    // (with an extension fallback), and it becomes an Image or Document block.
    let message = Message::user_with_files("Describe what you see in detail.", &paths)?;

    // Equivalent lower-level constructors, for reference:
    //   let block = ContentBlock::from_path("diagram.png")?;
    //   let block = ContentBlock::image_bytes("image/png", &std::fs::read("x.png")?);
    //   let block = ContentBlock::image_url("https://example.com/cat.jpg");
    //   let msg   = Message::user_with_media("caption", vec![block]);

    // ── Resolve the model from ~/.bricks/providers.toml ─────────────────────
    let provider = cersei::provider_from_config(None, &selection)?;
    let model = selection.as_str();

    let request = CompletionRequest {
        model: model.to_string(),
        messages: vec![message],
        system: Some("You are a careful visual analyst.".into()),
        tools: Vec::new(),
        max_tokens: 1024,
        temperature: None,
        stop_sequences: Vec::new(),
        options: ProviderOptions::default(),
        output_modalities: Vec::new(),
    };

    println!("─── Sending {} file(s) to {model} ───", paths.len());
    let response = provider.complete(request).await?.collect().await?;

    println!("{}", response.message.get_all_text());
    println!("─── Usage ───");
    println!("Input tok:  {}", response.usage.input_tokens);
    println!("Output tok: {}", response.usage.output_tokens);

    Ok(())
}
