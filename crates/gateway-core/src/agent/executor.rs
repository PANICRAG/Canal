//! Step executor for running task plan steps

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

use super::code_orchestration::runtime::CodeOrchestrationRuntime;
use super::code_orchestration::types::CodeOrchestrationRequest;
use super::planner::{PlanStep, StepAction, TaskPlan};
use super::worker::manager::WorkerManager;
use super::worker::types::WorkerSpecJson;
use crate::chat::StreamEvent;
use crate::error::{Error, Result};
use crate::llm::{ChatRequest, LlmRouter, Message};
use crate::mcp::McpGateway;

/// Result of executing a single step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step_id: Uuid,
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub summary: String,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Result of executing an entire plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub plan_id: Uuid,
    pub success: bool,
    pub step_results: Vec<StepResult>,
    pub final_output: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// Step executor
pub struct StepExecutor {
    llm_router: Arc<LlmRouter>,
    mcp_gateway: Arc<McpGateway>,
    /// Unified Tool System (preferred over mcp_gateway when available)
    tool_system: Option<Arc<crate::tool_system::ToolSystem>>,
    worker_manager: Option<Arc<WorkerManager>>,
    code_orchestration: Option<Arc<CodeOrchestrationRuntime>>,
}

impl StepExecutor {
    /// Create a new step executor
    pub fn new(llm_router: Arc<LlmRouter>, mcp_gateway: Arc<McpGateway>) -> Self {
        Self {
            llm_router,
            mcp_gateway,
            tool_system: None,
            worker_manager: None,
            code_orchestration: None,
        }
    }

    /// Set the unified tool system
    pub fn with_tool_system(mut self, tool_system: Arc<crate::tool_system::ToolSystem>) -> Self {
        self.tool_system = Some(tool_system);
        self
    }

    /// Set the worker manager for SpawnWorkers steps
    pub fn with_worker_manager(mut self, manager: Arc<WorkerManager>) -> Self {
        self.worker_manager = Some(manager);
        self
    }

    /// Set the code orchestration runtime for CodeOrchestration steps
    pub fn with_code_orchestration(mut self, runtime: Arc<CodeOrchestrationRuntime>) -> Self {
        self.code_orchestration = Some(runtime);
        self
    }

    /// Execute a complete task plan
    pub async fn execute(
        &self,
        plan: &TaskPlan,
        stream_tx: Option<broadcast::Sender<StreamEvent>>,
    ) -> Result<ExecutionResult> {
        let mut step_results = Vec::new();
        let mut context: HashMap<String, serde_json::Value> = HashMap::new();

        for step in &plan.steps {
            // Send step start event
            if let Some(ref tx) = stream_tx {
                let _ = tx.send(StreamEvent::tool_start(
                    &step.description,
                    &step.description,
                ));
            }

            let start_time = std::time::Instant::now();

            // Execute the step
            let result = self.execute_step(step, &context).await;

            let duration_ms = start_time.elapsed().as_millis() as u64;

            let step_result = match result {
                Ok(output) => {
                    // Store output for subsequent steps
                    let output_key = format!("step_{}_output", step.order);
                    if let Some(ref out) = output {
                        context.insert(output_key, out.clone());
                    }

                    // Send success event
                    if let Some(ref tx) = stream_tx {
                        let _ = tx.send(StreamEvent::tool_result_legacy(
                            &step.description,
                            true,
                            "Completed successfully",
                        ));
                    }

                    StepResult {
                        step_id: step.id,
                        success: true,
                        output,
                        summary: format!("Step {} completed", step.order),
                        error: None,
                        duration_ms,
                    }
                }
                Err(e) => {
                    // Send error event
                    if let Some(ref tx) = stream_tx {
                        let _ = tx.send(StreamEvent::tool_result_legacy(
                            &step.description,
                            false,
                            e.to_string(),
                        ));
                    }

                    StepResult {
                        step_id: step.id,
                        success: false,
                        output: None,
                        summary: format!("Step {} failed", step.order),
                        error: Some(e.to_string()),
                        duration_ms,
                    }
                }
            };

            let failed = !step_result.success;
            step_results.push(step_result);

            // Stop execution if step failed
            if failed {
                return Ok(ExecutionResult {
                    plan_id: plan.id,
                    success: false,
                    step_results,
                    final_output: None,
                    error: Some("Step execution failed".into()),
                });
            }
        }

        // Get final output from last step
        let final_output = step_results.last().and_then(|r| r.output.clone());

        Ok(ExecutionResult {
            plan_id: plan.id,
            success: true,
            step_results,
            final_output,
            error: None,
        })
    }

    /// Execute a single step
    async fn execute_step(
        &self,
        step: &PlanStep,
        context: &HashMap<String, serde_json::Value>,
    ) -> Result<Option<serde_json::Value>> {
        match &step.action {
            StepAction::LlmGenerate {
                prompt_template,
                model,
            } => {
                self.execute_llm_step(prompt_template, model.as_deref(), context)
                    .await
            }

            StepAction::McpCall { server, tool, args } => {
                self.execute_mcp_step(server, tool, args, context).await
            }

            StepAction::HttpCall {
                method,
                url,
                headers,
                body,
            } => {
                self.execute_http_step(method, url, headers.as_ref(), body.as_ref())
                    .await
            }

            StepAction::Transform {
                operation,
                input_ref,
                params,
            } => self.execute_transform_step(operation, input_ref, params, context),

            StepAction::WaitForInput { prompt: _ } => {
                // This should be handled by the orchestrator
                Ok(Some(serde_json::json!({ "status": "waiting_for_input" })))
            }

            StepAction::Condition { .. } => {
                // Conditions should be handled by the orchestrator
                Ok(Some(serde_json::json!({ "status": "condition_evaluated" })))
            }

            StepAction::SpawnWorkers {
                workers,
                synthesize_results,
                synthesis_prompt,
            } => {
                self.execute_spawn_workers_step(
                    workers,
                    *synthesize_results,
                    synthesis_prompt.as_deref(),
                    context,
                )
                .await
            }

            StepAction::CodeOrchestration {
                code,
                language,
                context_refs,
                timeout_ms,
            } => {
                self.execute_code_orchestration_step(
                    code,
                    language,
                    context_refs,
                    *timeout_ms,
                    context,
                )
                .await
            }
        }
    }

    /// Execute an LLM generation step
    async fn execute_llm_step(
        &self,
        prompt_template: &str,
        model: Option<&str>,
        context: &HashMap<String, serde_json::Value>,
    ) -> Result<Option<serde_json::Value>> {
        // Substitute context variables in prompt
        let mut prompt = prompt_template.to_string();
        for (key, value) in context {
            let placeholder = format!("{{{{{}}}}}", key);
            if let Some(text) = value.as_str() {
                prompt = prompt.replace(&placeholder, text);
            } else {
                prompt = prompt.replace(&placeholder, &value.to_string());
            }
        }

        let request = ChatRequest {
            messages: vec![Message::text("user", prompt)],
            model: model.map(|m| m.to_string()),
            max_tokens: Some(4096),
            temperature: Some(0.7),
            stream: false,
            ..Default::default()
        };

        let response = self.llm_router.route(request).await?;

        let content = response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        Ok(Some(serde_json::json!({
            "content": content,
            "model": response.model,
            "tokens": response.usage.total_tokens
        })))
    }

    /// Execute an MCP tool call step
    async fn execute_mcp_step(
        &self,
        server: &str,
        tool: &str,
        args: &serde_json::Value,
        _context: &HashMap<String, serde_json::Value>,
    ) -> Result<Option<serde_json::Value>> {
        // Call the MCP tool (prefer ToolSystem when available)
        let result = if let Some(ref ts) = self.tool_system {
            ts.execute(server, tool, args.clone()).await?
        } else {
            self.mcp_gateway
                .call_tool(server, tool, args.clone())
                .await?
        };
        Ok(Some(result))
    }

    /// Check if a URL is safe for outbound HTTP requests (SSRF prevention).
    fn is_safe_outbound_url(url: &str) -> Result<()> {
        let parsed =
            url::Url::parse(url).map_err(|e| Error::InvalidInput(format!("Invalid URL: {}", e)))?;

        // Only allow http/https schemes
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(Error::InvalidInput(format!(
                "Unsupported URL scheme: {}",
                parsed.scheme()
            )));
        }

        // R1-C1: Reject private/loopback/metadata addresses using typed host parsing.
        // Previous approach used string prefix matching which incorrectly blocked
        // public 172.200-255.x.x ranges and missed IPv6/IPv4-mapped addresses.
        match parsed.host() {
            Some(url::Host::Ipv4(v4)) => {
                if v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_broadcast()
                {
                    return Err(Error::InvalidInput(format!(
                        "URL targets private/internal address: {}",
                        v4
                    )));
                }
            }
            Some(url::Host::Ipv6(v6)) => {
                let is_private = v6.is_loopback()
                    || v6.is_unspecified()
                    || {
                        // Check IPv4-mapped IPv6 (::ffff:x.x.x.x)
                        v6.to_ipv4_mapped().map_or(false, |v4| {
                            v4.is_loopback()
                                || v4.is_private()
                                || v4.is_link_local()
                                || v4.is_unspecified()
                        })
                    }
                    || {
                        // fc00::/7 (unique local) and fe80::/10 (link-local)
                        let first = v6.segments()[0];
                        (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
                    };
                if is_private {
                    return Err(Error::InvalidInput(format!(
                        "URL targets private/internal address: {}",
                        v6
                    )));
                }
            }
            Some(url::Host::Domain(domain)) => {
                if matches!(domain, "localhost" | "0.0.0.0")
                    || domain.ends_with(".local")
                    || domain.ends_with(".internal")
                {
                    return Err(Error::InvalidInput(format!(
                        "URL targets private/internal address: {}",
                        domain
                    )));
                }
            }
            None => {
                return Err(Error::InvalidInput("URL has no host".to_string()));
            }
        }

        Ok(())
    }

    /// Execute an HTTP call step
    async fn execute_http_step(
        &self,
        method: &str,
        url: &str,
        headers: Option<&serde_json::Value>,
        body: Option<&serde_json::Value>,
    ) -> Result<Option<serde_json::Value>> {
        // SSRF protection: reject private/loopback/metadata URLs
        Self::is_safe_outbound_url(url)?;

        // R1-M: Reuse shared client for connection pooling and TLS session resumption
        static HTTP_CLIENT: std::sync::LazyLock<reqwest::Client> =
            std::sync::LazyLock::new(reqwest::Client::new);
        let client = &*HTTP_CLIENT;

        let mut request = match method.to_uppercase().as_str() {
            "GET" => client.get(url),
            "POST" => {
                let mut req = client.post(url);
                if let Some(b) = body {
                    req = req.json(b);
                }
                req
            }
            "PUT" => {
                let mut req = client.put(url);
                if let Some(b) = body {
                    req = req.json(b);
                }
                req
            }
            "DELETE" => client.delete(url),
            _ => {
                return Err(Error::Internal(format!(
                    "Unsupported HTTP method: {}",
                    method
                )))
            }
        };

        // R1-M: Apply caller-provided headers instead of silently dropping them
        if let Some(hdrs) = headers {
            if let Some(obj) = hdrs.as_object() {
                for (key, val) in obj {
                    if let Some(v) = val.as_str() {
                        request = request.header(key.as_str(), v);
                    }
                }
            }
        }

        // R1-M: Add 30s timeout to prevent indefinite hang on tarpit servers
        let response = request
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await?;
        let status = response.status();
        let body_text = response.text().await.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "Failed to read HTTP response body");
            String::new()
        });

        // Try to parse as JSON
        let body_json: serde_json::Value =
            serde_json::from_str(&body_text).unwrap_or(serde_json::json!({ "text": body_text }));

        Ok(Some(serde_json::json!({
            "status": status.as_u16(),
            "body": body_json
        })))
    }

    /// Execute a SpawnWorkers step using the WorkerManager
    async fn execute_spawn_workers_step(
        &self,
        workers: &[WorkerSpecJson],
        synthesize_results: bool,
        synthesis_prompt: Option<&str>,
        _context: &HashMap<String, serde_json::Value>,
    ) -> Result<Option<serde_json::Value>> {
        let manager = self.worker_manager.as_ref().ok_or_else(|| {
            Error::Worker("WorkerManager not configured. Cannot execute SpawnWorkers step.".into())
        })?;

        // Convert JSON specs to WorkerSpecs
        let mut name_to_id = std::collections::HashMap::new();
        for spec in workers {
            name_to_id.insert(spec.name.clone(), Uuid::new_v4());
        }

        let worker_specs: Vec<super::worker::types::WorkerSpec> = workers
            .iter()
            .map(|json_spec| {
                let mut spec = json_spec.to_worker_spec(&name_to_id);
                if let Some(&id) = name_to_id.get(&json_spec.name) {
                    spec.id = id;
                }
                spec
            })
            .collect();

        // Execute workers
        let mut result = manager.execute_workers(worker_specs, None).await?;

        // Optionally synthesize
        if synthesize_results && !result.worker_results.is_empty() {
            let synthesized = manager
                .synthesize_results(&result.worker_results, synthesis_prompt)
                .await?;
            result.synthesized_output = Some(synthesized);
        }

        Ok(Some(serde_json::to_value(&result).unwrap_or_default()))
    }

    /// Execute a CodeOrchestration step using the CodeOrchestrationRuntime
    async fn execute_code_orchestration_step(
        &self,
        code: &str,
        language: &str,
        context_refs: &[String],
        timeout_ms: Option<u64>,
        context: &HashMap<String, serde_json::Value>,
    ) -> Result<Option<serde_json::Value>> {
        let runtime = self.code_orchestration.as_ref().ok_or_else(|| {
            Error::CodeOrchestration(
                "CodeOrchestrationRuntime not configured. Cannot execute CodeOrchestration step."
                    .into(),
            )
        })?;

        // Build context data from references
        let mut context_data = serde_json::Map::new();
        for ref_key in context_refs {
            if let Some(value) = context.get(ref_key) {
                context_data.insert(ref_key.clone(), value.clone());
            }
        }

        let request = CodeOrchestrationRequest {
            code: code.to_string(),
            language: language.to_string(),
            context_refs: context_refs.to_vec(),
            context_data: serde_json::Value::Object(context_data),
            timeout: timeout_ms
                .map(std::time::Duration::from_millis)
                .unwrap_or(std::time::Duration::from_secs(300)),
        };

        // R1-M: Use system temp dir instead of hardcoded /tmp (platform-portable)
        let tool_context =
            crate::agent::tools::ToolContext::new("code-orchestration", &std::env::temp_dir());

        let result = runtime.execute(request, tool_context).await?;

        Ok(Some(serde_json::to_value(&result).unwrap_or_default()))
    }

    /// Execute a transform step
    fn execute_transform_step(
        &self,
        operation: &str,
        input_ref: &str,
        params: &serde_json::Value,
        context: &HashMap<String, serde_json::Value>,
    ) -> Result<Option<serde_json::Value>> {
        let input = context.get(input_ref);

        match operation {
            "identity" => Ok(input.cloned()),
            "extract" => {
                // Extract a field from JSON
                let field = params["field"].as_str().unwrap_or("");
                if let Some(input_val) = input {
                    Ok(Some(input_val[field].clone()))
                } else {
                    Ok(None)
                }
            }
            "format" => {
                // Format as string
                let template = params["template"].as_str().unwrap_or("{{input}}");
                let formatted = if let Some(input_val) = input {
                    template.replace("{{input}}", &input_val.to_string())
                } else {
                    template.to_string()
                };
                Ok(Some(serde_json::json!({ "formatted": formatted })))
            }
            "merge" => {
                // Merge multiple inputs
                let refs: Vec<&str> = params["refs"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                let mut merged = serde_json::Map::new();
                for r in refs {
                    if let Some(val) = context.get(r) {
                        if let Some(obj) = val.as_object() {
                            for (k, v) in obj {
                                merged.insert(k.clone(), v.clone());
                            }
                        }
                    }
                }
                Ok(Some(serde_json::Value::Object(merged)))
            }
            _ => Ok(input.cloned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transform_extract() {
        let llm_router = Arc::new(LlmRouter::new(crate::llm::LlmConfig::default()));
        let mcp_gateway = Arc::new(McpGateway::new());
        let executor = StepExecutor::new(llm_router, mcp_gateway);

        let mut context = HashMap::new();
        context.insert(
            "input".to_string(),
            serde_json::json!({
                "name": "test",
                "value": 42
            }),
        );

        let result = executor
            .execute_transform_step(
                "extract",
                "input",
                &serde_json::json!({ "field": "name" }),
                &context,
            )
            .unwrap();

        assert_eq!(result, Some(serde_json::json!("test")));
    }

    // R1-C1: SSRF prevention tests
    #[test]
    fn test_ssrf_blocks_private_ipv4() {
        assert!(StepExecutor::is_safe_outbound_url("http://127.0.0.1/admin").is_err());
        assert!(StepExecutor::is_safe_outbound_url("http://10.0.0.1/secret").is_err());
        assert!(StepExecutor::is_safe_outbound_url("http://192.168.1.1/").is_err());
        assert!(StepExecutor::is_safe_outbound_url("http://172.16.0.1/").is_err());
        assert!(StepExecutor::is_safe_outbound_url("http://172.31.255.255/").is_err());
        assert!(StepExecutor::is_safe_outbound_url("http://169.254.169.254/metadata").is_err());
    }

    #[test]
    fn test_ssrf_allows_public_ips() {
        assert!(StepExecutor::is_safe_outbound_url("https://8.8.8.8/").is_ok());
        // 172.32+ is public — previous bug blocked these
        assert!(StepExecutor::is_safe_outbound_url("http://172.32.0.1/").is_ok());
        assert!(StepExecutor::is_safe_outbound_url("http://172.200.0.1/").is_ok());
        assert!(StepExecutor::is_safe_outbound_url("https://1.1.1.1/").is_ok());
    }

    #[test]
    fn test_ssrf_blocks_ipv6_private() {
        assert!(StepExecutor::is_safe_outbound_url("http://[::1]/").is_err());
        // IPv4-mapped IPv6
        assert!(StepExecutor::is_safe_outbound_url("http://[::ffff:127.0.0.1]/").is_err());
        assert!(StepExecutor::is_safe_outbound_url("http://[::ffff:10.0.0.1]/").is_err());
    }

    #[test]
    fn test_ssrf_blocks_special_hosts() {
        assert!(StepExecutor::is_safe_outbound_url("http://localhost/").is_err());
        assert!(StepExecutor::is_safe_outbound_url("http://foo.local/").is_err());
        assert!(StepExecutor::is_safe_outbound_url("http://bar.internal/").is_err());
    }

    #[test]
    fn test_ssrf_rejects_non_http_schemes() {
        assert!(StepExecutor::is_safe_outbound_url("ftp://example.com/").is_err());
        assert!(StepExecutor::is_safe_outbound_url("file:///etc/passwd").is_err());
    }
}
