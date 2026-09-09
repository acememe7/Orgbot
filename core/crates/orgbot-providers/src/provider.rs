//! The backend trait.

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::error::Result;
use crate::types::{CompletionRequest, CompletionResponse, StreamEvent};

/// Resolve bounded local image references only for the outgoing API request.
pub(crate) async fn prepare_images(
    mut request: CompletionRequest,
    vision: bool,
) -> Result<CompletionRequest> {
    use crate::{types::ImageSource, ContentBlock, ProviderError};
    use base64::Engine as _;
    use tokio::io::AsyncReadExt;
    let mut images = 0;
    // ponytail: keep the eight newest images in each request; retain older file
    // references in saved memory so the original can still be inspected by tools.
    for turn in request.messages.iter_mut().rev() {
        for block in turn.content.iter_mut().rev() {
            let ContentBlock::Image { media_type, source } = block else {
                continue;
            };
            if !vision {
                *block = ContentBlock::text("An image was attached, but this provider accepts text only. Ask a vision-capable teammate to inspect it; do not claim to have seen it.");
                continue;
            }
            images += 1;
            if images > 8 {
                *block = ContentBlock::text("An older image was omitted from this request's eight-image limit. Its original attachment remains available through workspace_import_attachment; ask for relevant pages if needed.");
                continue;
            }
            if !matches!(
                media_type.as_str(),
                "image/jpeg" | "image/png" | "image/webp" | "image/gif"
            ) {
                return Err(ProviderError::Unsupported(
                    "Unsupported image format".into(),
                ));
            }
            if let ImageSource::File { path } = source {
                let file = tokio::fs::File::open(path).await.map_err(|_| {
                    ProviderError::Unsupported(
                        "An attached image is missing; attach it again".into(),
                    )
                })?;
                let mut bytes = Vec::new();
                file.take(2 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| {
                        ProviderError::Unsupported("Could not read an attached image".into())
                    })?;
                if bytes.len() > 2 * 1024 * 1024 || bytes.is_empty() {
                    return Err(ProviderError::Unsupported(
                        "An image exceeds 2 MiB or is empty".into(),
                    ));
                }
                *source = ImageSource::Base64 {
                    data: base64::engine::general_purpose::STANDARD.encode(bytes),
                };
            }
        }
    }
    Ok(request)
}

/// A model backend.
///
/// One method covers both modes: pass a `sink` to receive incremental updates
/// while the turn runs, or `None` to just wait for the result. Either way the
/// return value is the complete assembled turn, so the agent loop does not care
/// whether the UI was watching.
#[async_trait]
pub trait Provider: Send + Sync + std::fmt::Debug {
    /// A stable name for logs and error messages.
    fn name(&self) -> &str;

    /// Run one model turn.
    async fn complete(
        &self,
        request: CompletionRequest,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Result<CompletionResponse>;
}

/// Emit a stream event, ignoring a closed receiver.
///
/// A dropped UI subscriber must not abort a turn that is otherwise fine, so
/// send failures here are deliberately swallowed.
pub(crate) async fn emit(sink: &Option<mpsc::Sender<StreamEvent>>, event: StreamEvent) {
    if let Some(tx) = sink {
        let _ = tx.send(event).await;
    }
}

/// Allow long reasoning turns while bounding a stalled network. Room controls
/// cancel the in-flight future independently of these network deadlines.
pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        // Keep idle reasoning connections alive across mobile routers, and
        // detect a lost peer instead of waiting for the OS's long defaults.
        .tcp_keepalive(std::time::Duration::from_secs(30))
        .tcp_keepalive_interval(std::time::Duration::from_secs(10))
        .tcp_keepalive_retries(3)
        .read_timeout(std::time::Duration::from_secs(300))
        .timeout(std::time::Duration::from_secs(1800))
        .build()
        .expect("valid HTTP client settings")
}

#[cfg(test)]
mod attachment_tests {
    use super::*;
    use crate::{
        types::ImageSource, AnthropicProvider, CodexProvider, ContentBlock, Credential,
        OpenAiProvider, Turn,
    };

    #[tokio::test]
    async fn images_reach_all_wire_formats_without_bloating_memory_or_text_only_requests() {
        let path =
            std::env::temp_dir().join(format!("orgbot-image-{}", orgbot_proto::RoomId::new()));
        std::fs::write(&path, [1u8, 2, 3]).unwrap();
        let mut turn = Turn::user("Read the attached research image");
        turn.content.push(ContentBlock::Image {
            media_type: "image/jpeg".into(),
            source: ImageSource::File { path: path.clone() },
        });
        let original = CompletionRequest::new("model", "system", vec![turn]);
        let request = prepare_images(original.clone(), true).await.unwrap();
        let codex = CodexProvider::new("https://example.test", Credential::Bearer("test".into()))
            .build_body(&request);
        assert_eq!(codex["input"][0]["content"][1]["type"], "input_image");
        assert_eq!(
            codex["input"][0]["content"][1]["image_url"],
            "data:image/jpeg;base64,AQID"
        );
        let grok = OpenAiProvider::grok_with_credential(Credential::Bearer("test".into()))
            .build_body(&request);
        assert_eq!(
            grok["messages"][1]["content"][1]["image_url"]["url"],
            "data:image/jpeg;base64,AQID"
        );
        let claude = AnthropicProvider::with_credential(Credential::ApiKey("test".into()))
            .build_body(&request);
        assert_eq!(
            claude["messages"][0]["content"][1]["source"]["data"],
            "AQID"
        );
        assert!(matches!(
            original.messages[0].content[1],
            ContentBlock::Image {
                source: ImageSource::File { .. },
                ..
            }
        ));
        let mut many = original.clone();
        many.messages = vec![original.messages[0].clone(); 10];
        let bounded = prepare_images(many, true).await.unwrap();
        assert!(matches!(
            bounded.messages[0].content[1],
            ContentBlock::Text { .. }
        ));
        assert_eq!(
            bounded
                .messages
                .iter()
                .flat_map(|t| &t.content)
                .filter(|b| matches!(b, ContentBlock::Image { .. }))
                .count(),
            8
        );
        std::fs::remove_file(&path).unwrap();
        let text_only = prepare_images(original.clone(), false).await.unwrap();
        assert!(text_only.messages[0].text().contains("text only"));
        assert!(prepare_images(original.clone(), true).await.is_err());
        std::fs::write(&path, vec![0u8; 2 * 1024 * 1024 + 1]).unwrap();
        assert!(prepare_images(original, true).await.is_err());
        std::fs::remove_file(path).unwrap();
    }
}
