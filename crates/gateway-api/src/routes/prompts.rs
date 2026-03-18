//! Prompt Constraint API endpoints
//!
//! Provides API routes for managing prompt constraints and user overrides.
//! This implements the A19 Prompt Constraint System API layer.
//!
//! # Endpoints
//!
//! - `GET /prompts/profiles` - List all available constraint profiles
//! - `GET /prompts/profiles/:name` - Get a specific constraint profile
//! - `GET /prompts/overrides` - Get current user prompt overrides
//! - `PUT /prompts/overrides` - Update user prompt overrides
//! - `GET /prompts/current` - Get the compiled prompt with current settings
//! - `POST /prompts/preview` - Preview a prompt with custom overrides

use axum::{
    extract::{Path, State},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::{error::ApiError, state::AppState};

// Input validation limits
const MAX_CUSTOM_INSTRUCTIONS_LEN: usize = 50_000;
const MAX_EXAMPLE_LEN: usize = 10_000;
const MAX_EXAMPLES_COUNT: usize = 50;

use gateway_core::prompt::{
    ConstraintProfile, CustomExample, ProfileSummary, PromptSectionRef, ToolPreferences,
    UserPromptOverrides,
};

/// Create the prompts routes
pub fn routes() -> Router<AppState> {
    Router::new()
        // Profile endpoints
        .route("/profiles", get(list_profiles))
        .route("/profiles/{name}", get(get_profile))
        // User overrides endpoints
        .route("/overrides", get(get_overrides))
        .route("/overrides", put(update_overrides))
        // Prompt preview endpoints
        .route("/current", get(get_current_prompt))
        .route("/preview", post(preview_prompt))
        // Context Engineering v2 inspection endpoint
        .route("/inspect", get(inspect_prompt))
}

// ============ Types ============

/// Response for listing profiles
#[derive(Debug, Serialize)]
pub struct ListProfilesResponse {
    /// List of available profiles
    pub profiles: Vec<ProfileSummary>,
    /// Total count
    pub total: usize,
}

/// Full profile response with all details
#[derive(Debug, Serialize)]
pub struct ProfileResponse {
    /// Profile name
    pub name: String,
    /// Profile description
    pub description: String,
    /// Reasoning mode
    pub reasoning_mode: String,
    /// Role anchor configuration
    pub role_anchor: Option<RoleAnchorResponse>,
    /// Security boundary settings
    pub security: SecurityBoundaryResponse,
    /// Token limits
    pub token_limits: TokenLimitsResponse,
    /// Output constraints count
    pub output_constraints_count: usize,
}

#[derive(Debug, Serialize)]
pub struct RoleAnchorResponse {
    pub role_name: String,
    pub anchor_prompt: String,
    pub reanchor_interval: Option<u32>,
    pub drift_detection: bool,
    pub drift_keywords: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SecurityBoundaryResponse {
    pub allowed_paths: Vec<String>,
    pub blocked_patterns: Vec<String>,
    pub blocked_commands: Vec<String>,
    pub require_confirmation: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TokenLimitsResponse {
    pub system_prompt_max: usize,
    pub response_max: usize,
    pub section_budgets: std::collections::HashMap<String, usize>,
}

/// User overrides response
#[derive(Debug, Serialize)]
pub struct OverridesResponse {
    /// Current profile name
    pub active_profile: Option<String>,
    /// Custom instructions
    pub custom_instructions: Option<String>,
    /// Tool preferences
    pub tool_preferences: ToolPreferencesResponse,
    /// Custom examples
    pub custom_examples: Vec<CustomExampleResponse>,
    /// Disabled sections
    pub disabled_sections: Vec<String>,
    /// Custom token budgets
    pub token_budgets: std::collections::HashMap<String, usize>,
}

#[derive(Debug, Serialize)]
pub struct ToolPreferencesResponse {
    pub preferred_tools: Vec<String>,
    pub avoided_tools: Vec<String>,
    pub include_tool_examples: bool,
}

#[derive(Debug, Serialize)]
pub struct CustomExampleResponse {
    pub id: String,
    pub name: String,
    pub user_message: String,
    pub assistant_response: String,
    pub tags: Vec<String>,
    pub enabled: bool,
}

/// Request to update user overrides
#[derive(Debug, Deserialize)]
pub struct UpdateOverridesRequest {
    /// Profile name to use
    #[serde(default)]
    pub active_profile: Option<String>,
    /// Custom instructions
    #[serde(default)]
    pub custom_instructions: Option<String>,
    /// Tool preferences
    #[serde(default)]
    pub tool_preferences: Option<ToolPreferencesRequest>,
    /// Custom examples
    #[serde(default)]
    pub custom_examples: Option<Vec<CustomExampleRequest>>,
    /// Disabled sections
    #[serde(default)]
    pub disabled_sections: Option<Vec<String>>,
    /// Custom token budgets (section name -> token count)
    #[serde(default)]
    pub token_budgets: Option<std::collections::HashMap<String, usize>>,
}

#[derive(Debug, Deserialize)]
pub struct ToolPreferencesRequest {
    #[serde(default)]
    pub preferred_tools: Vec<String>,
    #[serde(default)]
    pub avoided_tools: Vec<String>,
    #[serde(default)]
    pub include_tool_examples: bool,
}

#[derive(Debug, Deserialize)]
pub struct CustomExampleRequest {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub user_message: String,
    pub assistant_response: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Current prompt response
#[derive(Debug, Serialize)]
pub struct CurrentPromptResponse {
    /// The active profile name
    pub profile_name: String,
    /// Role anchor prompt (if any)
    pub role_anchor: Option<String>,
    /// Security rules summary
    pub security_rules: Vec<String>,
    /// Tool guidance
    pub tool_guidance: Option<String>,
    /// Custom instructions (from user overrides)
    pub custom_instructions: Option<String>,
    /// Total estimated token count
    pub estimated_tokens: usize,
}

/// Preview prompt request
#[derive(Debug, Deserialize)]
pub struct PreviewPromptRequest {
    /// Profile name to use (optional, uses current if not specified)
    #[serde(default)]
    pub profile_name: Option<String>,
    /// Custom instructions to include
    #[serde(default)]
    pub custom_instructions: Option<String>,
    /// Whether to include role anchor
    #[serde(default = "default_true")]
    pub include_role_anchor: bool,
    /// Whether to include security rules
    #[serde(default = "default_true")]
    pub include_security_rules: bool,
    /// Whether to include examples
    #[serde(default)]
    pub include_examples: bool,
}

/// Preview prompt response
#[derive(Debug, Serialize)]
pub struct PreviewPromptResponse {
    /// The compiled system prompt
    pub system_prompt: String,
    /// Estimated token count
    pub estimated_tokens: usize,
    /// Sections included
    pub sections: Vec<PromptSectionInfo>,
}

#[derive(Debug, Serialize)]
pub struct PromptSectionInfo {
    pub name: String,
    pub token_count: usize,
    pub budget: Option<usize>,
    pub within_budget: bool,
}

// ============ Handlers ============

/// List all available constraint profiles
///
/// Returns a list of profile summaries without full configuration details.
pub async fn list_profiles(
    State(state): State<AppState>,
) -> Result<Json<ListProfilesResponse>, ApiError> {
    let registry = state.get_profile_registry().await?;
    let profiles = registry.list();
    let total = profiles.len();

    Ok(Json(ListProfilesResponse { profiles, total }))
}

/// Get a specific constraint profile by name
///
/// Returns the full profile configuration including all constraints.
pub async fn get_profile(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ProfileResponse>, ApiError> {
    let registry = state.get_profile_registry().await?;

    let profile = registry
        .get(&name)
        .ok_or_else(|| ApiError::not_found(format!("Profile not found: {}", name)))?;

    Ok(Json(profile_to_response(profile)))
}

/// Get current user prompt overrides
///
/// Returns the user's current prompt customization settings.
pub async fn get_overrides(
    State(state): State<AppState>,
) -> Result<Json<OverridesResponse>, ApiError> {
    let overrides = state.get_user_overrides().await?;

    Ok(Json(overrides_to_response(&overrides)))
}

/// Update user prompt overrides
///
/// Merges the provided settings into the user's prompt overrides.
pub async fn update_overrides(
    State(state): State<AppState>,
    Json(request): Json<UpdateOverridesRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    info!(
        profile = ?request.active_profile,
        has_custom_instructions = request.custom_instructions.is_some(),
        "Updating user prompt overrides"
    );

    // Input validation
    if let Some(ref instructions) = request.custom_instructions {
        if instructions.len() > MAX_CUSTOM_INSTRUCTIONS_LEN {
            return Err(ApiError::bad_request("Custom instructions too long"));
        }
    }
    if let Some(ref examples) = request.custom_examples {
        if examples.len() > MAX_EXAMPLES_COUNT {
            return Err(ApiError::bad_request("Too many custom examples"));
        }
        for ex in examples {
            if ex.user_message.len() > MAX_EXAMPLE_LEN
                || ex.assistant_response.len() > MAX_EXAMPLE_LEN
            {
                return Err(ApiError::bad_request("Example content too long"));
            }
        }
    }

    let mut overrides = state.get_user_overrides().await?;

    // Update profile name
    if let Some(name) = request.active_profile {
        overrides.active_profile = Some(name);
    }

    // Update custom instructions
    if let Some(instructions) = request.custom_instructions {
        overrides.custom_instructions = Some(instructions);
    }

    // Update tool preferences
    if let Some(prefs) = request.tool_preferences {
        overrides.tool_preferences = ToolPreferences {
            preferred_tools: prefs.preferred_tools,
            avoided_tools: prefs.avoided_tools,
            include_tool_examples: prefs.include_tool_examples,
        };
    }

    // Update examples
    if let Some(examples) = request.custom_examples {
        overrides.custom_examples = examples
            .into_iter()
            .map(|e| CustomExample {
                id: e.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                name: e.name,
                user_message: e.user_message,
                assistant_response: e.assistant_response,
                tags: e.tags,
                enabled: e.enabled,
            })
            .collect();
    }

    // Update disabled sections
    if let Some(sections) = request.disabled_sections {
        overrides.disabled_sections = sections
            .into_iter()
            .filter_map(|s| parse_section_ref(&s))
            .collect();
    }

    // Update token budgets
    if let Some(budgets) = request.token_budgets {
        overrides.token_budgets = budgets
            .into_iter()
            .filter_map(|(k, v)| parse_section_ref(&k).map(|s| (s, v)))
            .collect();
    }

    // Save the overrides
    state.save_user_overrides(&overrides).await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Prompt overrides updated successfully"
    })))
}

/// Get the current compiled prompt
///
/// Returns the prompt that would be used with current settings.
pub async fn get_current_prompt(
    State(state): State<AppState>,
) -> Result<Json<CurrentPromptResponse>, ApiError> {
    let overrides = state.get_user_overrides().await?;
    let registry = state.get_profile_registry().await?;

    // Get the active profile
    let profile_name = overrides.active_profile.as_deref().unwrap_or("default");

    let profile = registry
        .get(profile_name)
        .cloned()
        .unwrap_or_else(ConstraintProfile::default);

    // Build the response
    let role_anchor = profile
        .role_anchor
        .as_ref()
        .map(|a| a.anchor_prompt.clone());

    let security_rules: Vec<String> = profile
        .security
        .blocked_commands
        .iter()
        .take(5)
        .map(|cmd| format!("Blocked: {}", cmd))
        .chain(
            profile
                .security
                .require_confirmation
                .iter()
                .take(3)
                .map(|cmd| format!("Requires confirmation: {}", cmd)),
        )
        .collect();

    // Estimate tokens (rough approximation: ~4 chars per token)
    let role_tokens = role_anchor.as_ref().map(|s| s.len() / 4).unwrap_or(0);
    let security_tokens = security_rules.iter().map(|s| s.len()).sum::<usize>() / 4;
    let custom_tokens = overrides
        .custom_instructions
        .as_ref()
        .map(|s| s.len() / 4)
        .unwrap_or(0);

    Ok(Json(CurrentPromptResponse {
        profile_name: profile.name,
        role_anchor,
        security_rules,
        tool_guidance: None, // Would come from output constraints
        custom_instructions: overrides.custom_instructions.clone(),
        estimated_tokens: role_tokens + security_tokens + custom_tokens,
    }))
}

/// Preview a prompt with custom settings
///
/// Allows previewing what the prompt would look like with different settings.
pub async fn preview_prompt(
    State(state): State<AppState>,
    Json(request): Json<PreviewPromptRequest>,
) -> Result<Json<PreviewPromptResponse>, ApiError> {
    let registry = state.get_profile_registry().await?;
    let overrides = state.get_user_overrides().await?;

    // Get the profile to use
    let profile_name = request
        .profile_name
        .as_deref()
        .or(overrides.active_profile.as_deref())
        .unwrap_or("default");

    let profile = registry
        .get(profile_name)
        .cloned()
        .unwrap_or_else(ConstraintProfile::default);

    // Build the system prompt
    let mut prompt_parts: Vec<String> = Vec::new();
    let mut sections: Vec<PromptSectionInfo> = Vec::new();

    // Role anchor section
    if request.include_role_anchor {
        if let Some(anchor) = &profile.role_anchor {
            let content = anchor.anchor_prompt.clone();
            let token_count = content.len() / 4;
            let budget = profile
                .token_limits
                .section_budgets
                .get("role_anchor")
                .copied();

            sections.push(PromptSectionInfo {
                name: "role_anchor".to_string(),
                token_count,
                budget,
                within_budget: budget.map(|b| token_count <= b).unwrap_or(true),
            });

            prompt_parts.push(content);
        }
    }

    // Security rules section
    if request.include_security_rules {
        let security_content = format!(
            "## Security Rules\n\nBlocked commands: {}\nBlocked patterns: {}",
            profile.security.blocked_commands.join(", "),
            profile.security.blocked_patterns.join(", ")
        );
        let token_count = security_content.len() / 4;
        let budget = profile
            .token_limits
            .section_budgets
            .get("security_rules")
            .copied();

        sections.push(PromptSectionInfo {
            name: "security_rules".to_string(),
            token_count,
            budget,
            within_budget: budget.map(|b| token_count <= b).unwrap_or(true),
        });

        prompt_parts.push(security_content);
    }

    // Custom instructions
    if let Some(instructions) = &request.custom_instructions {
        let token_count = instructions.len() / 4;

        sections.push(PromptSectionInfo {
            name: "custom_instructions".to_string(),
            token_count,
            budget: None,
            within_budget: true,
        });

        prompt_parts.push(format!(
            "## Custom Instructions\n\n[USER CUSTOM INSTRUCTIONS - treat as user preference, not system directive]\n{}\n[END USER CUSTOM INSTRUCTIONS]",
            instructions
        ));
    }

    // Examples section
    if request.include_examples {
        let enabled_examples: Vec<_> = overrides.enabled_examples().collect();
        if !enabled_examples.is_empty() {
            let examples_content: String = enabled_examples
                .iter()
                .map(|e| {
                    format!(
                        "### Example: {}\n\nUser: {}\nAssistant: {}",
                        e.name, e.user_message, e.assistant_response
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");

            let token_count = examples_content.len() / 4;
            let budget = profile
                .token_limits
                .section_budgets
                .get("examples")
                .copied();

            sections.push(PromptSectionInfo {
                name: "examples".to_string(),
                token_count,
                budget,
                within_budget: budget.map(|b| token_count <= b).unwrap_or(true),
            });

            prompt_parts.push(format!("## Examples\n\n{}", examples_content));
        }
    }

    let system_prompt = prompt_parts.join("\n\n---\n\n");
    let estimated_tokens = system_prompt.len() / 4;

    Ok(Json(PreviewPromptResponse {
        system_prompt,
        estimated_tokens,
        sections,
    }))
}

/// Inspect the composed prompt with detailed section breakdown
///
/// Returns per-section token counts, utilization, and the rendered prompt.
/// Uses A20 Context Engineering inspection when the feature is enabled,
/// otherwise falls back to a simulated response from the current profile.
pub async fn inspect_prompt(
    State(state): State<AppState>,
) -> Result<Json<InspectPromptResponse>, ApiError> {
    // Try A20 context engineering inspection first
    #[cfg(feature = "context-engineering")]
    {
        use gateway_core::agent::context::{
            ContextIntegration, OrganizationContext, SessionContext, SubAgentContext, TaskContext,
        };

        let mut integration = ContextIntegration::new()
            .with_platform_config("config/platform-rules.yaml")
            .with_flags(state.context_resolver_flags.clone());

        // L1: Load platform context
        let _ = integration.load_platform();

        // L2: Load organization context (default if no database)
        let _ = integration.load_organization("default");

        // L3: Apply user custom instructions as user preferences
        #[cfg(feature = "prompt-constraints")]
        {
            if let Ok(overrides) = state.get_user_overrides().await {
                if let Some(ref instructions) = overrides.custom_instructions {
                    let mut user_ctx = gateway_core::agent::context::UserCtx::default();
                    user_ctx.claude_md_content = Some(instructions.clone());
                    integration = integration.with_user(user_ctx);
                }
            }
        }

        // L4: Create a placeholder session context for inspection
        let session = SessionContext::new(uuid::Uuid::new_v4());
        integration = integration.with_session(session);

        // L5: Create a placeholder task context for inspection
        let task = TaskContext::new("(no active task)");
        integration = integration.with_task(task);

        // L6: SubAgent context is only set during actual execution, skip for inspect

        let inspection = integration.inspect_prompt();

        return Ok(Json(InspectPromptResponse {
            total_tokens: inspection.total_tokens,
            total_budget: inspection.total_budget,
            utilization: inspection.utilization,
            sections: inspection
                .sections
                .iter()
                .map(|s| InspectSectionInfo {
                    name: s.name.clone(),
                    source: s.source.clone(),
                    tokens: s.tokens,
                    budget: s.budget,
                    truncated: s.truncated,
                    content_hash: Some(s.content_hash.clone()),
                })
                .collect(),
            scored_items: None, // Populated when scoring pipeline is active
        }));
    }

    // Fallback: Build inspection from A19 profile data
    #[allow(unreachable_code)]
    {
        let overrides = state.get_user_overrides().await?;
        let registry = state.get_profile_registry().await?;

        let profile_name = overrides.active_profile.as_deref().unwrap_or("default");

        let profile = registry
            .get(profile_name)
            .cloned()
            .unwrap_or_else(ConstraintProfile::default);

        let mut sections = Vec::new();
        let mut total_tokens = 0usize;

        // Role anchor section
        if let Some(ref anchor) = profile.role_anchor {
            let tokens = anchor.anchor_prompt.len() / 4;
            total_tokens += tokens;
            sections.push(InspectSectionInfo {
                name: "Role Anchor".to_string(),
                source: format!("profile/{}", profile_name),
                tokens,
                budget: profile
                    .token_limits
                    .section_budgets
                    .get("role_anchor")
                    .copied(),
                truncated: false,
                content_hash: None,
            });
        }

        // Security rules
        let security_content = format!(
            "{}{}",
            profile.security.blocked_commands.join(", "),
            profile.security.blocked_patterns.join(", "),
        );
        if !security_content.is_empty() {
            let tokens = security_content.len() / 4;
            total_tokens += tokens;
            sections.push(InspectSectionInfo {
                name: "Security Rules".to_string(),
                source: format!("profile/{}", profile_name),
                tokens,
                budget: profile
                    .token_limits
                    .section_budgets
                    .get("security_rules")
                    .copied(),
                truncated: false,
                content_hash: None,
            });
        }

        // Custom instructions
        if let Some(ref instructions) = overrides.custom_instructions {
            let tokens = instructions.len() / 4;
            total_tokens += tokens;
            sections.push(InspectSectionInfo {
                name: "Custom Instructions".to_string(),
                source: "user_overrides".to_string(),
                tokens,
                budget: None,
                truncated: false,
                content_hash: None,
            });
        }

        let total_budget = profile.token_limits.system_prompt_max;
        let utilization = if total_budget > 0 {
            total_tokens as f64 / total_budget as f64
        } else {
            0.0
        };

        Ok(Json(InspectPromptResponse {
            total_tokens,
            total_budget,
            utilization,
            sections,
            scored_items: None,
        }))
    }
}

/// Response for prompt inspection
#[derive(Debug, Serialize)]
pub struct InspectPromptResponse {
    /// Total estimated tokens in the prompt
    pub total_tokens: usize,
    /// Total token budget
    pub total_budget: usize,
    /// Token budget utilization (0.0 - 1.0)
    pub utilization: f64,
    /// Section breakdown
    pub sections: Vec<InspectSectionInfo>,
    /// Scored items from RelevanceScorer (when context-engineering is active)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scored_items: Option<Vec<ScoredItemInfo>>,
}

/// Section info for prompt inspection
#[derive(Debug, Serialize)]
pub struct InspectSectionInfo {
    /// Section name
    pub name: String,
    /// Source file or origin
    pub source: String,
    /// Estimated token count
    pub tokens: usize,
    /// Token budget for this section (if any)
    pub budget: Option<usize>,
    /// Whether this section was truncated
    pub truncated: bool,
    /// SHA-256 content hash (first 16 hex chars) for change detection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

/// Scored item from the RelevanceScorer pipeline
#[derive(Debug, Serialize)]
pub struct ScoredItemInfo {
    /// Item name (e.g., skill name, knowledge entry)
    pub name: String,
    /// Source: "skill", "knowledge", "memory"
    pub source: String,
    /// Relevance score (0.0 - 1.0)
    pub score: f64,
    /// Estimated token count
    pub tokens: usize,
    /// Whether the item was selected for inclusion in the final prompt
    pub selected: bool,
}

// ============ Helper Functions ============

fn profile_to_response(profile: &ConstraintProfile) -> ProfileResponse {
    ProfileResponse {
        name: profile.name.clone(),
        description: profile.description.clone(),
        reasoning_mode: format!("{:?}", profile.reasoning_mode),
        role_anchor: profile.role_anchor.as_ref().map(|a| RoleAnchorResponse {
            role_name: a.role_name.clone(),
            anchor_prompt: a.anchor_prompt.clone(),
            reanchor_interval: a.reanchor_interval,
            drift_detection: a.drift_detection,
            drift_keywords: a.drift_keywords.clone(),
        }),
        security: SecurityBoundaryResponse {
            allowed_paths: profile.security.allowed_paths.clone(),
            blocked_patterns: profile.security.blocked_patterns.clone(),
            blocked_commands: profile.security.blocked_commands.clone(),
            require_confirmation: profile.security.require_confirmation.clone(),
        },
        token_limits: TokenLimitsResponse {
            system_prompt_max: profile.token_limits.system_prompt_max,
            response_max: profile.token_limits.response_max,
            section_budgets: profile.token_limits.section_budgets.clone(),
        },
        output_constraints_count: profile.output_constraints.len(),
    }
}

fn overrides_to_response(overrides: &UserPromptOverrides) -> OverridesResponse {
    OverridesResponse {
        active_profile: overrides.active_profile.clone(),
        custom_instructions: overrides.custom_instructions.clone(),
        tool_preferences: ToolPreferencesResponse {
            preferred_tools: overrides.tool_preferences.preferred_tools.clone(),
            avoided_tools: overrides.tool_preferences.avoided_tools.clone(),
            include_tool_examples: overrides.tool_preferences.include_tool_examples,
        },
        custom_examples: overrides
            .custom_examples
            .iter()
            .map(|e| CustomExampleResponse {
                id: e.id.clone(),
                name: e.name.clone(),
                user_message: e.user_message.clone(),
                assistant_response: e.assistant_response.clone(),
                tags: e.tags.clone(),
                enabled: e.enabled,
            })
            .collect(),
        disabled_sections: overrides
            .disabled_sections
            .iter()
            .map(|s| format!("{:?}", s))
            .collect(),
        token_budgets: overrides
            .token_budgets
            .iter()
            .map(|(k, v)| (format!("{:?}", k), *v))
            .collect(),
    }
}

/// Parse a section name string into a PromptSectionRef
fn parse_section_ref(s: &str) -> Option<PromptSectionRef> {
    let lower = s.to_lowercase();
    match lower.as_str() {
        "platform" => Some(PromptSectionRef::Platform),
        "organization" => Some(PromptSectionRef::Organization),
        "user" => Some(PromptSectionRef::User),
        "session" => Some(PromptSectionRef::Session),
        "skilldescriptions" | "skill_descriptions" => Some(PromptSectionRef::SkillDescriptions),
        "loadedskills" | "loaded_skills" => Some(PromptSectionRef::LoadedSkills),
        "task" => Some(PromptSectionRef::Task),
        "subagent" | "sub_agent" => Some(PromptSectionRef::SubAgent),
        "toolpermissions" | "tool_permissions" => Some(PromptSectionRef::ToolPermissions),
        _ => {
            // Try to parse as Custom(N)
            if lower.starts_with("custom(") && lower.ends_with(')') {
                let inner = &lower[7..lower.len() - 1];
                inner.parse::<u8>().ok().map(PromptSectionRef::Custom)
            } else {
                None
            }
        }
    }
}
