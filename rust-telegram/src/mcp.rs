use std::collections::HashSet;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::auth::{Access, Result, nonce, response};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Tool {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default = "default_input_schema")]
    pub(crate) input_schema: Value,
}

fn default_input_schema() -> Value {
    json!({"type":"object"})
}

#[derive(Deserialize)]
struct Catalog {
    tools: Vec<Tool>,
    #[serde(rename = "nextCursor")]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct RpcResponse {
    result: Option<Value>,
    error: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolResponse {
    is_error: Option<bool>,
    structured_content: Option<Value>,
    content: Option<Vec<Value>>,
}

pub(crate) struct Mcp<'a> {
    http: &'a Client,
    origin: &'a Url,
    access: &'a Access,
}

impl<'a> Mcp<'a> {
    pub(crate) fn new(http: &'a Client, origin: &'a Url, access: &'a Access) -> Self {
        Self {
            http,
            origin,
            access,
        }
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let request = match self.access {
            Access::Student(_) => return Err("student phone grant does not authorize MCP".into()),
            Access::Teacher { token, secret } => self
                .http
                .post(self.origin.join("/api/telegram/teacher-mcp")?)
                .bearer_auth(secret)
                .header("x-telegram-grant", token),
            Access::OAuth(token) => self.http.post(self.origin.join("/mcp")?).bearer_auth(token),
        };
        let body: RpcResponse = response(
            request
                .header("accept", "application/json, text/event-stream")
                .json(&json!({
                    "jsonrpc": "2.0", "id": nonce::<18>()?, "method": method, "params": params
                })),
        )
        .await?;
        if body.error.is_some() {
            return Err("MCP request failed".into());
        }
        body.result.ok_or_else(|| "missing MCP result".into())
    }

    pub(crate) async fn tools(&self) -> Result<HashSet<String>> {
        Ok(self
            .catalog()
            .await?
            .into_iter()
            .map(|tool| tool.name)
            .collect())
    }

    pub(crate) async fn catalog(&self) -> Result<Vec<Tool>> {
        let mut names = HashSet::new();
        let mut seen = HashSet::new();
        let mut collected = Vec::new();
        let mut params = json!({});
        for _ in 0..10 {
            let catalog: Catalog =
                serde_json::from_value(self.request("tools/list", params).await?)?;
            if catalog.tools.len() > 200 {
                return Err("MCP catalog page too large".into());
            }
            for tool in catalog.tools {
                if tool.name.is_empty()
                    || tool.name.len() > 100
                    || !names.insert(tool.name.clone())
                    || tool.description.encode_utf16().count() > 4000
                    || tool.input_schema["type"] != "object"
                {
                    return Err("invalid MCP tool definition".into());
                }
                collected.push(tool);
            }
            if names.len() > 500 {
                return Err("MCP catalog too large".into());
            }
            match catalog.next_cursor {
                None => return Ok(collected),
                Some(cursor)
                    if !cursor.is_empty()
                        && cursor.len() <= 1000
                        && seen.insert(cursor.clone()) =>
                {
                    params = json!({"cursor": cursor});
                }
                Some(_) => return Err("invalid MCP cursor".into()),
            }
        }
        Err("MCP catalog exceeds page limit".into())
    }

    pub(crate) async fn call(&self, name: &str, args: Value) -> Result<Value> {
        let body: ToolResponse = serde_json::from_value(
            self.request(
                "tools/call",
                json!({
                    "name": name, "arguments": args,
                }),
            )
            .await?,
        )?;
        if body.is_error == Some(true) {
            return Err("MCP tool failed".into());
        }
        if let Some(content) = body.structured_content {
            return Ok(content);
        }
        let content = body.content.unwrap_or_default();
        for item in &content {
            if item["type"] == "resource"
                && item["resource"]["mimeType"]
                    .as_str()
                    .is_some_and(|t| t.contains("spreadsheet"))
            {
                if let Some(blob) = item["resource"]["blob"].as_str() {
                    return Ok(json!({ "blob": blob }));
                }
            }
        }
        for item in &content {
            if item["type"] == "text" {
                if let Some(text) = item["text"].as_str() {
                    return Ok(serde_json::from_str(text).unwrap_or_else(|_| json!(text)));
                }
            }
        }
        Err("missing MCP tool content".into())
    }
}
