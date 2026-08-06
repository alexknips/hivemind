//! Shared HTTP client for the Anthropic messages API (Layer-3 workers).

use serde::{Deserialize, Serialize};

pub(crate) const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub(crate) const API_VERSION: &str = "2023-06-01";

pub(crate) type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Serialize)]
struct Request {
    model: &'static str,
    max_tokens: u32,
    output_config: OutputConfig,
    messages: Vec<Message>,
}

#[derive(Debug, Serialize)]
struct OutputConfig {
    format: Format,
}

#[derive(Debug, Serialize)]
struct Format {
    #[serde(rename = "type")]
    format_type: &'static str,
    schema: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct Message {
    role: &'static str,
    content: String,
}

#[derive(Debug, Deserialize)]
struct Response {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

/// Send a single-turn request to the Anthropic messages API with structured
/// JSON output. `schema` must be a JSON Schema object; the response text is
/// deserialized into `T`.
pub(crate) async fn call_json_schema<T>(
    client: &reqwest::Client,
    api_key: &str,
    model: &'static str,
    max_tokens: u32,
    user_content: String,
    schema: serde_json::Value,
) -> Result<T, BoxError>
where
    T: serde::de::DeserializeOwned,
{
    let request = Request {
        model,
        max_tokens,
        output_config: OutputConfig {
            format: Format {
                format_type: "json_schema",
                schema,
            },
        },
        messages: vec![Message {
            role: "user",
            content: user_content,
        }],
    };

    let response = client
        .post(API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", API_VERSION)
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Anthropic API returned {status}: {body}").into());
    }

    let api_resp: Response = response.json().await?;
    let text = api_resp
        .content
        .into_iter()
        .find(|b| b.block_type == "text")
        .and_then(|b| b.text)
        .ok_or("no text block in Anthropic API response")?;

    let output: T = serde_json::from_str(&text)?;
    Ok(output)
}
