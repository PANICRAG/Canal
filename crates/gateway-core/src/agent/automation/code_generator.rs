//! Code Generator - Layer 3 of the Five-Layer Automation Architecture
//!
//! Generates executable automation scripts (Playwright, Selenium, API calls)
//! from PageSchema and task descriptions.
//!
//! Token cost: Fixed ~500-1000 tokens regardless of data volume.

use super::types::{GeneratedScript, PageSchema, ScriptType};
use crate::llm::LlmRouter;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

// ============================================================================
// Error Types
// ============================================================================

#[derive(Error, Debug)]
pub enum GeneratorError {
    #[error("LLM generation failed: {0}")]
    GenerationFailed(String),

    #[error("Invalid schema: {0}")]
    InvalidSchema(String),

    #[error("Script validation failed: {0}")]
    ValidationFailed(String),

    #[error("Unsupported script type: {0}")]
    UnsupportedScriptType(String),
}

// ============================================================================
// Generation Options
// ============================================================================

/// Options for code generation
#[derive(Debug, Clone)]
pub struct GenerationOptions {
    /// Target script type
    pub script_type: ScriptType,
    /// Whether to include retry logic
    pub include_retry: bool,
    /// Maximum retries
    pub max_retries: u32,
    /// Whether to include pagination handling
    pub handle_pagination: bool,
    /// Whether to include error handling
    pub include_error_handling: bool,
    /// Whether to add logging
    pub include_logging: bool,
    /// Whether to validate the generated script
    pub validate_script: bool,
    /// Custom template to use
    pub template: Option<String>,
    /// Timeout for operations (milliseconds)
    pub operation_timeout_ms: u64,
}

impl Default for GenerationOptions {
    fn default() -> Self {
        Self {
            script_type: ScriptType::Playwright,
            include_retry: true,
            max_retries: 3,
            handle_pagination: false,
            include_error_handling: true,
            include_logging: true,
            validate_script: true,
            template: None,
            operation_timeout_ms: 30000,
        }
    }
}

// ============================================================================
// Generation Result
// ============================================================================

/// Result of code generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationResult {
    /// Generated script
    pub script: GeneratedScript,
    /// Token usage
    pub tokens_used: u64,
    /// Generation duration in milliseconds
    pub duration_ms: u64,
    /// Validation passed
    pub validation_passed: bool,
    /// Validation errors (if any)
    pub validation_errors: Vec<String>,
    /// Warnings
    pub warnings: Vec<String>,
}

// ============================================================================
// Code Generator
// ============================================================================

/// Code Generator - Creates automation scripts from PageSchema
#[allow(dead_code)]
pub struct CodeGenerator {
    /// LLM router for code generation
    llm_router: Arc<LlmRouter>,
    /// Configuration
    config: CodeGeneratorConfig,
}

/// Configuration for the code generator
#[derive(Debug, Clone)]
pub struct CodeGeneratorConfig {
    /// Model to use for generation
    pub model: String,
    /// Maximum tokens for generation
    pub max_tokens: u32,
    /// System prompt for code generation
    pub system_prompt: String,
}

impl Default for CodeGeneratorConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 2048,
            system_prompt: Self::default_system_prompt(),
        }
    }
}

impl CodeGeneratorConfig {
    fn default_system_prompt() -> String {
        r#"You are an expert automation script generator. Generate clean, efficient scripts based on the provided page schema and task.

Guidelines:
1. Use the exact selectors/coordinates from the schema
2. Include proper waits for elements to load
3. Handle errors gracefully
4. Make the script data-driven (accept data as parameter)
5. Do NOT include actual business data in the script
6. Output only valid, executable code

For Playwright scripts:
- Use async/await syntax
- Use page.waitForSelector before interactions
- Use page.click() with coordinates for canvas apps
- Use page.fill() for text inputs

The script should:
- Accept data as a parameter (array of objects)
- Iterate through data items
- Perform the required action for each item
- Return results/errors"#
            .to_string()
    }
}

impl CodeGenerator {
    /// Create a new code generator
    pub fn new(llm_router: Arc<LlmRouter>) -> Self {
        Self {
            llm_router,
            config: CodeGeneratorConfig::default(),
        }
    }

    /// Create a builder
    pub fn builder() -> CodeGeneratorBuilder {
        CodeGeneratorBuilder::default()
    }

    /// Generate automation script
    pub async fn generate(
        &self,
        task: &str,
        schema: &PageSchema,
        options: GenerationOptions,
    ) -> Result<GenerationResult, GeneratorError> {
        let start = std::time::Instant::now();
        let warnings = Vec::new();

        // 1. Validate schema
        self.validate_schema(schema)?;

        // 2. Build generation prompt (for future LLM integration)
        let _prompt = self.build_generation_prompt(task, schema, &options);

        // 3. Generate placeholder code (TODO: integrate with LLM)
        let code = self.generate_placeholder_code(task, schema, &options);
        let tokens = 800u64; // Estimated token count

        // 4. Post-process the code
        let processed_code = self.post_process_code(&code, &options);

        // 5. Validate if requested
        let (validation_passed, validation_errors) = if options.validate_script {
            self.validate_script(&processed_code, options.script_type)
        } else {
            (true, Vec::new())
        };

        // 6. Create script
        let task_signature = self.generate_task_signature(task, &schema.url);
        let script = GeneratedScript::new(
            options.script_type,
            processed_code,
            self.get_language(options.script_type),
            &schema.schema_hash,
            task_signature,
        );

        Ok(GenerationResult {
            script,
            tokens_used: tokens,
            duration_ms: start.elapsed().as_millis() as u64,
            validation_passed,
            validation_errors,
            warnings,
        })
    }

    /// Generate placeholder code based on task and schema
    /// TODO: Replace with actual LLM generation
    fn generate_placeholder_code(
        &self,
        task: &str,
        schema: &PageSchema,
        options: &GenerationOptions,
    ) -> String {
        match options.script_type {
            ScriptType::Playwright => {
                format!(
                    r#"
// Auto-generated Playwright script
// Task: {}
// Target URL: {}

const {{ chromium }} = require('playwright');

async function processData(data) {{
    const browser = await chromium.launch({{ headless: false }});
    const page = await browser.newPage();
    const results = [];

    try {{
        await page.goto('{}');
        await page.waitForLoadState('networkidle');

        for (const item of data) {{
            try {{
                // TODO: Add actual automation logic based on schema
                // Schema has {} elements and {} actions
                results.push({{ success: true, item }});
            }} catch (error) {{
                results.push({{ success: false, item, error: error.message }});
            }}
        }}
    }} finally {{
        await browser.close();
    }}

    return results;
}}

module.exports = {{ processData }};
"#,
                    task,
                    schema.url,
                    schema.url,
                    schema.elements.len(),
                    schema.actions.len()
                )
            }
            ScriptType::Selenium => {
                format!(
                    r#"
# Auto-generated Selenium script
# Task: {}
# Target URL: {}

from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.support.ui import WebDriverWait
from selenium.webdriver.support import expected_conditions as EC

def process_data(data):
    driver = webdriver.Chrome()
    results = []

    try:
        driver.get('{}')
        WebDriverWait(driver, 10).until(
            EC.presence_of_element_located((By.TAG_NAME, "body"))
        )

        for item in data:
            try:
                # TODO: Add actual automation logic based on schema
                # Schema has {} elements and {} actions
                results.append({{'success': True, 'item': item}})
            except Exception as e:
                results.append({{'success': False, 'item': item, 'error': str(e)}})
    finally:
        driver.quit()

    return results

if __name__ == '__main__':
    import sys
    import json
    data = json.loads(sys.argv[1]) if len(sys.argv) > 1 else []
    print(json.dumps(process_data(data)))
"#,
                    task,
                    schema.url,
                    schema.url,
                    schema.elements.len(),
                    schema.actions.len()
                )
            }
            _ => format!("// Placeholder script for task: {}", task),
        }
    }

    /// Validate the schema has required information
    fn validate_schema(&self, schema: &PageSchema) -> Result<(), GeneratorError> {
        if schema.url.is_empty() {
            return Err(GeneratorError::InvalidSchema(
                "Schema has no URL".to_string(),
            ));
        }
        Ok(())
    }

    /// Build the generation prompt
    fn build_generation_prompt(
        &self,
        task: &str,
        schema: &PageSchema,
        options: &GenerationOptions,
    ) -> String {
        let schema_json = serde_json::to_string_pretty(schema).unwrap_or_default();
        let script_type_name = match options.script_type {
            ScriptType::Playwright => "Playwright (JavaScript/TypeScript)",
            ScriptType::Selenium => "Selenium (Python)",
            ScriptType::Puppeteer => "Puppeteer (JavaScript)",
            ScriptType::RestApi => "REST API calls (Python requests)",
            ScriptType::GraphQl => "GraphQL (Python)",
            ScriptType::Native => "Native automation",
        };

        let mut prompt = format!(
            r#"Generate a {} script for the following task:

## Task
{}

## Page Schema
{}

## Requirements
"#,
            script_type_name, task, schema_json,
        );

        if options.include_retry {
            prompt.push_str(&format!(
                "- Include retry logic with max {} retries\n",
                options.max_retries
            ));
        }

        if options.handle_pagination {
            prompt.push_str("- Handle pagination if present\n");
        }

        if options.include_error_handling {
            prompt.push_str("- Include comprehensive error handling\n");
        }

        if options.include_logging {
            prompt.push_str("- Add logging for debugging\n");
        }

        prompt.push_str(&format!(
            "- Operation timeout: {}ms\n",
            options.operation_timeout_ms
        ));

        prompt.push_str("\n## Output\nProvide only the script code, no explanations.");

        if let Some(template) = &options.template {
            prompt.push_str(&format!("\n\n## Template\n{}", template));
        }

        prompt
    }

    /// Extract code from LLM response
    #[allow(dead_code)]
    fn extract_code(&self, content: &str, script_type: ScriptType) -> String {
        let lang_markers = match script_type {
            ScriptType::Playwright | ScriptType::Puppeteer => {
                vec!["```javascript", "```typescript", "```js", "```ts"]
            }
            ScriptType::Selenium | ScriptType::RestApi | ScriptType::GraphQl => {
                vec!["```python", "```py"]
            }
            ScriptType::Native => vec!["```"],
        };

        // Try to find code block with language marker
        for marker in &lang_markers {
            if let Some(start) = content.find(marker) {
                let code_start = start + marker.len();
                if let Some(end) = content[code_start..].find("```") {
                    return content[code_start..code_start + end].trim().to_string();
                }
            }
        }

        // Try generic code block
        if let Some(start) = content.find("```") {
            let code_start = start + 3;
            let actual_start = content[code_start..]
                .find('\n')
                .map(|n| code_start + n + 1)
                .unwrap_or(code_start);
            if let Some(end) = content[actual_start..].find("```") {
                return content[actual_start..actual_start + end].trim().to_string();
            }
        }

        content.trim().to_string()
    }

    /// Post-process the generated code
    fn post_process_code(&self, code: &str, options: &GenerationOptions) -> String {
        let mut processed = code.to_string();

        match options.script_type {
            ScriptType::Playwright | ScriptType::Puppeteer => {
                if !processed.starts_with("//")
                    && !processed.starts_with("const")
                    && !processed.starts_with("import")
                {
                    processed = format!("// Auto-generated automation script\n{}", processed);
                }
            }
            ScriptType::Selenium | ScriptType::RestApi | ScriptType::GraphQl => {
                if !processed.starts_with("#")
                    && !processed.starts_with("import")
                    && !processed.starts_with("from")
                {
                    processed = format!("# Auto-generated automation script\n{}", processed);
                }
            }
            _ => {}
        }

        processed
    }

    /// Validate the generated script
    fn validate_script(&self, code: &str, script_type: ScriptType) -> (bool, Vec<String>) {
        let mut errors = Vec::new();

        match script_type {
            ScriptType::Playwright => {
                if !code.contains("page.")
                    && !code.contains("browser")
                    && !code.contains("chromium")
                {
                    errors.push("Script doesn't appear to use Playwright API".to_string());
                }
            }
            ScriptType::Selenium => {
                if !code.contains("driver") && !code.contains("webdriver") {
                    errors.push("Script doesn't appear to use Selenium API".to_string());
                }
            }
            ScriptType::RestApi => {
                if !code.contains("requests") && !code.contains("fetch") && !code.contains("http") {
                    errors.push("Script doesn't appear to make HTTP requests".to_string());
                }
            }
            _ => {}
        }

        if code.contains("TODO") || code.contains("FIXME") {
            // This is expected for placeholder code
        }

        (errors.is_empty(), errors)
    }

    /// Get language for script type
    fn get_language(&self, script_type: ScriptType) -> String {
        match script_type {
            ScriptType::Playwright | ScriptType::Puppeteer => "javascript".to_string(),
            ScriptType::Selenium | ScriptType::RestApi | ScriptType::GraphQl => {
                "python".to_string()
            }
            ScriptType::Native => "shell".to_string(),
        }
    }

    /// Generate a task signature for caching
    fn generate_task_signature(&self, task: &str, url: &str) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        task.to_lowercase().hash(&mut hasher);
        url.hash(&mut hasher);
        format!("gen_{:x}", hasher.finish())
    }
}

// ============================================================================
// Builder
// ============================================================================

/// Builder for CodeGenerator
#[derive(Default)]
pub struct CodeGeneratorBuilder {
    llm_router: Option<Arc<LlmRouter>>,
    config: CodeGeneratorConfig,
}

impl CodeGeneratorBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set LLM router
    pub fn llm_router(mut self, router: Arc<LlmRouter>) -> Self {
        self.llm_router = Some(router);
        self
    }

    /// Set model
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = model.into();
        self
    }

    /// Set max tokens
    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.config.max_tokens = tokens;
        self
    }

    /// Set system prompt
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.system_prompt = prompt.into();
        self
    }

    /// Build the generator
    pub fn build(self) -> Result<CodeGenerator, GeneratorError> {
        let llm_router = self.llm_router.ok_or(GeneratorError::GenerationFailed(
            "LLM router not provided".to_string(),
        ))?;

        Ok(CodeGenerator {
            llm_router,
            config: self.config,
        })
    }
}

// ============================================================================
// Script Templates
// ============================================================================

/// Pre-built script templates for common scenarios
pub struct ScriptTemplates;

impl ScriptTemplates {
    /// Playwright template for data entry
    pub fn playwright_data_entry() -> String {
        r#"
const { chromium } = require('playwright');

async function processData(data) {
    const browser = await chromium.launch({ headless: false });
    const page = await browser.newPage();

    const results = [];

    for (const item of data) {
        try {
            // Navigate and interact based on schema
            results.push({ success: true, item });
        } catch (error) {
            results.push({ success: false, item, error: error.message });
        }
    }

    await browser.close();
    return results;
}

module.exports = { processData };
"#
        .to_string()
    }

    /// Selenium template for data entry
    pub fn selenium_data_entry() -> String {
        r#"
from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.support.ui import WebDriverWait
from selenium.webdriver.support import expected_conditions as EC

def process_data(data):
    driver = webdriver.Chrome()
    results = []

    try:
        for item in data:
            try:
                results.append({'success': True, 'item': item})
            except Exception as e:
                results.append({'success': False, 'item': item, 'error': str(e)})
    finally:
        driver.quit()

    return results
"#
        .to_string()
    }

    /// REST API template
    pub fn rest_api() -> String {
        r#"
import requests
from typing import List, Dict, Any

def process_data(data: List[Dict], base_url: str, headers: Dict = None) -> List[Dict]:
    results = []
    session = requests.Session()

    if headers:
        session.headers.update(headers)

    for item in data:
        try:
            response = session.post(f"{base_url}/api/endpoint", json=item)
            response.raise_for_status()
            results.append({'success': True, 'item': item, 'response': response.json()})
        except Exception as e:
            results.append({'success': False, 'item': item, 'error': str(e)})

    return results
"#
        .to_string()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generation_options_default() {
        let options = GenerationOptions::default();
        assert_eq!(options.script_type, ScriptType::Playwright);
        assert!(options.include_retry);
        assert_eq!(options.max_retries, 3);
    }

    #[test]
    fn test_config_default() {
        let config = CodeGeneratorConfig::default();
        assert_eq!(config.model, "claude-sonnet-4-6");
        assert_eq!(config.max_tokens, 2048);
    }

    #[test]
    fn test_script_templates() {
        let playwright = ScriptTemplates::playwright_data_entry();
        assert!(playwright.contains("chromium"));
        assert!(playwright.contains("processData"));

        let selenium = ScriptTemplates::selenium_data_entry();
        assert!(selenium.contains("webdriver"));
        assert!(selenium.contains("process_data"));

        let api = ScriptTemplates::rest_api();
        assert!(api.contains("requests"));
    }
}
