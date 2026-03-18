//! CLI command implementations

use anyhow::Result;
use colored::Colorize;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Gateway API client
pub struct GatewayClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

impl GatewayClient {
    pub fn new(base_url: &str, api_key: Option<&str>) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.map(String::from),
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.request(method, &url);

        if let Some(key) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        req
    }
}

/// Chat with an LLM
pub async fn chat(client: &GatewayClient, message: &str, model: Option<&str>) -> Result<()> {
    println!("{}", "Sending chat request...".dimmed());

    #[derive(Serialize)]
    struct ChatRequest {
        messages: Vec<ChatMessage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    }

    #[derive(Serialize)]
    struct ChatMessage {
        role: String,
        content: String,
    }

    #[derive(Deserialize)]
    struct ChatResponse {
        choices: Vec<Choice>,
    }

    #[derive(Deserialize)]
    struct Choice {
        message: ResponseMessage,
    }

    #[derive(Deserialize)]
    struct ResponseMessage {
        content: String,
    }

    let request = ChatRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: message.to_string(),
        }],
        model: model.map(String::from),
    };

    let response = client
        .request(reqwest::Method::POST, "/api/chat")
        .json(&request)
        .send()
        .await?;

    if response.status().is_success() {
        let chat_response: ChatResponse = response.json().await?;
        if let Some(choice) = chat_response.choices.first() {
            println!("\n{}", "Assistant:".green().bold());
            println!("{}", choice.message.content);
        }
    } else {
        let status = response.status();
        let text = response.text().await?;
        // R9-H11: Return error instead of Ok(()) so CLI exits with non-zero code
        anyhow::bail!("HTTP {}: {}", status, text);
    }

    Ok(())
}

/// List available tools
pub async fn tools_list(client: &GatewayClient) -> Result<()> {
    println!("{}", "Fetching tools...".dimmed());

    #[derive(Deserialize)]
    struct ToolsResponse {
        tools: Vec<Tool>,
        count: usize,
    }

    #[derive(Deserialize)]
    struct Tool {
        name: String,
        namespace: String,
        description: String,
    }

    let response = client
        .request(reqwest::Method::GET, "/api/tools")
        .send()
        .await?;

    if response.status().is_success() {
        let tools_response: ToolsResponse = response.json().await?;
        println!(
            "\n{} ({})",
            "Available Tools".green().bold(),
            tools_response.count
        );
        for tool in tools_response.tools {
            println!(
                "  {} {}",
                format!("{}.{}", tool.namespace, tool.name).cyan(),
                format!("- {}", tool.description).dimmed()
            );
        }
    } else {
        let status = response.status();
        let text = response.text().await?;
        anyhow::bail!("HTTP {}: {}", status, text);
    }

    Ok(())
}

/// Call a tool
pub async fn tools_call(client: &GatewayClient, name: &str, input: Option<&str>) -> Result<()> {
    let parts: Vec<&str> = name.splitn(2, '.').collect();
    if parts.len() != 2 {
        // R9-H11: Return error for invalid input instead of Ok(())
        anyhow::bail!("Tool name must be in format: namespace.tool");
    }

    let (namespace, tool_name) = (parts[0], parts[1]);

    println!("{} {}.{}", "Calling tool:".dimmed(), namespace, tool_name);

    #[derive(Serialize)]
    struct ToolCallRequest {
        input: serde_json::Value,
    }

    let input_value: serde_json::Value = input
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    let request = ToolCallRequest { input: input_value };

    let response = client
        .request(
            reqwest::Method::POST,
            &format!("/api/tools/{}/{}/call", namespace, tool_name),
        )
        .json(&request)
        .send()
        .await?;

    if response.status().is_success() {
        let result: serde_json::Value = response.json().await?;
        println!("\n{}", "Result:".green().bold());
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        let status = response.status();
        let text = response.text().await?;
        anyhow::bail!("HTTP {}: {}", status, text);
    }

    Ok(())
}

/// List workflows
pub async fn workflows_list(client: &GatewayClient) -> Result<()> {
    println!("{}", "Fetching workflows...".dimmed());

    #[derive(Deserialize)]
    struct WorkflowsResponse {
        workflows: Vec<Workflow>,
        count: usize,
    }

    #[derive(Deserialize)]
    struct Workflow {
        id: String,
        name: String,
        #[allow(dead_code)]
        description: String,
        step_count: usize,
    }

    let response = client
        .request(reqwest::Method::GET, "/api/workflows")
        .send()
        .await?;

    if response.status().is_success() {
        let workflows_response: WorkflowsResponse = response.json().await?;
        println!(
            "\n{} ({})",
            "Workflows".green().bold(),
            workflows_response.count
        );
        for workflow in workflows_response.workflows {
            println!(
                "  {} {} ({} steps)",
                workflow.id.cyan(),
                format!("- {}", workflow.name).dimmed(),
                workflow.step_count
            );
        }
    } else {
        let status = response.status();
        let text = response.text().await?;
        anyhow::bail!("HTTP {}: {}", status, text);
    }

    Ok(())
}

/// Get workflow details
pub async fn workflows_get(client: &GatewayClient, id: &str) -> Result<()> {
    let response = client
        .request(reqwest::Method::GET, &format!("/api/workflows/{}", id))
        .send()
        .await?;

    if response.status().is_success() {
        let workflow: serde_json::Value = response.json().await?;
        println!("\n{}", "Workflow Details:".green().bold());
        println!("{}", serde_json::to_string_pretty(&workflow)?);
    } else {
        let status = response.status();
        let text = response.text().await?;
        anyhow::bail!("HTTP {}: {}", status, text);
    }

    Ok(())
}

/// Execute a workflow
pub async fn workflows_execute(
    client: &GatewayClient,
    id: &str,
    input: Option<&str>,
) -> Result<()> {
    println!("{} {}", "Executing workflow:".dimmed(), id);

    #[derive(Serialize)]
    struct ExecuteRequest {
        input: serde_json::Value,
    }

    let input_value: serde_json::Value = input
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    let request = ExecuteRequest { input: input_value };

    let response = client
        .request(
            reqwest::Method::POST,
            &format!("/api/workflows/{}/execute", id),
        )
        .json(&request)
        .send()
        .await?;

    if response.status().is_success() {
        let result: serde_json::Value = response.json().await?;
        println!("\n{}", "Execution Result:".green().bold());
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        let status = response.status();
        let text = response.text().await?;
        anyhow::bail!("HTTP {}: {}", status, text);
    }

    Ok(())
}

/// List MCP servers
pub async fn mcp_list(client: &GatewayClient) -> Result<()> {
    println!("{}", "Fetching MCP servers...".dimmed());

    let response = client
        .request(reqwest::Method::GET, "/api/mcp/servers")
        .send()
        .await?;

    if response.status().is_success() {
        let servers: serde_json::Value = response.json().await?;
        println!("\n{}", "MCP Servers:".green().bold());
        println!("{}", serde_json::to_string_pretty(&servers)?);
    } else {
        let status = response.status();
        let text = response.text().await?;
        anyhow::bail!("HTTP {}: {}", status, text);
    }

    Ok(())
}

/// Get MCP server status
pub async fn mcp_status(client: &GatewayClient, id: &str) -> Result<()> {
    let response = client
        .request(
            reqwest::Method::GET,
            &format!("/api/mcp/servers/{}/status", id),
        )
        .send()
        .await?;

    if response.status().is_success() {
        let status_info: serde_json::Value = response.json().await?;
        println!("\n{} {}", "MCP Server Status:".green().bold(), id);
        println!("{}", serde_json::to_string_pretty(&status_info)?);
    } else {
        let status = response.status();
        let text = response.text().await?;
        anyhow::bail!("HTTP {}: {}", status, text);
    }

    Ok(())
}

/// Check gateway health
pub async fn health(client: &GatewayClient) -> Result<()> {
    println!("{}", "Checking gateway health...".dimmed());

    let response = client
        .request(reqwest::Method::GET, "/api/health")
        .send()
        .await?;

    if response.status().is_success() {
        let health: serde_json::Value = response.json().await?;
        println!("\n{} {}", "Status:".green().bold(), "Healthy".green());
        println!("{}", serde_json::to_string_pretty(&health)?);
    } else {
        let status = response.status();
        println!(
            "\n{} {} ({})",
            "Status:".red().bold(),
            "Unhealthy".red(),
            status
        );
    }

    Ok(())
}
