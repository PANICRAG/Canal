//! Organization Context Loader
//!
//! Loads and manages organization-level context from database.
//! Organization context defines team-wide rules that apply to all members, including:
//! - Code style guidelines
//! - Naming conventions
//! - Team knowledge and best practices
//! - Shared memory for organizational learning

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::resolver::{ContextLayer, ContextPriority, ResolvedContext};
use crate::error::Error;

// ============================================================================
// Organization Context Types
// ============================================================================

/// Organization-level context configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizationContext {
    /// Organization unique identifier
    pub id: Uuid,

    /// Organization name
    pub name: String,

    /// Organization-specific rules and guidelines
    pub context_rules: OrganizationRules,

    /// Shared memory entries accessible to all organization members
    pub shared_memory: Vec<String>,
}

impl Default for OrganizationContext {
    fn default() -> Self {
        Self {
            id: Uuid::nil(),
            name: String::new(),
            context_rules: OrganizationRules::default(),
            shared_memory: Vec::new(),
        }
    }
}

/// Organization rules and guidelines
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OrganizationRules {
    /// Code style rules for the organization
    pub code_style: Option<CodeStyleRules>,

    /// Convention rules (commits, branches, etc.)
    pub conventions: Option<ConventionRules>,

    /// Team knowledge entries
    pub team_knowledge: Vec<String>,
}

/// Code style rules for the organization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeStyleRules {
    /// Primary programming language
    pub language: String,

    /// Formatting tool/style (e.g., "rustfmt", "prettier")
    pub formatting: Option<String>,

    /// Testing framework/approach
    pub testing: Option<String>,

    /// Linting rules
    pub linting: Option<String>,

    /// Documentation style
    pub documentation: Option<String>,
}

impl Default for CodeStyleRules {
    fn default() -> Self {
        Self {
            language: String::new(),
            formatting: None,
            testing: None,
            linting: None,
            documentation: None,
        }
    }
}

/// Convention rules for the organization
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConventionRules {
    /// Commit message prefix (e.g., "[JIRA-123]", "feat:", etc.)
    pub commit_prefix: Option<String>,

    /// Branch naming pattern (e.g., "feature/", "bugfix/")
    pub branch_pattern: Option<String>,

    /// PR title format
    pub pr_title_format: Option<String>,

    /// Code review requirements
    pub review_requirements: Option<String>,
}

// ============================================================================
// ContextLayer Implementation
// ============================================================================

impl ContextLayer for OrganizationContext {
    fn layer_name(&self) -> &str {
        "organization"
    }

    fn priority(&self) -> ContextPriority {
        ContextPriority::Organization
    }

    fn apply_to(&self, resolved: &mut ResolvedContext) {
        // Generate org conventions string
        let mut conventions = String::new();
        conventions.push_str(&format!("## {} Organization Conventions\n\n", self.name));

        // Add code style rules
        if let Some(code_style) = &self.context_rules.code_style {
            conventions.push_str("### Code Style\n");
            conventions.push_str(&format!("- Language: {}\n", code_style.language));

            if let Some(fmt) = &code_style.formatting {
                conventions.push_str(&format!("- Formatting: {}\n", fmt));
            }

            if let Some(testing) = &code_style.testing {
                conventions.push_str(&format!("- Testing: {}\n", testing));
            }

            if let Some(linting) = &code_style.linting {
                conventions.push_str(&format!("- Linting: {}\n", linting));
            }

            if let Some(docs) = &code_style.documentation {
                conventions.push_str(&format!("- Documentation: {}\n", docs));
            }

            conventions.push('\n');
        }

        // Add convention rules
        if let Some(conv) = &self.context_rules.conventions {
            conventions.push_str("### Conventions\n");

            if let Some(prefix) = &conv.commit_prefix {
                conventions.push_str(&format!("- Commit prefix: {}\n", prefix));
            }

            if let Some(pattern) = &conv.branch_pattern {
                conventions.push_str(&format!("- Branch pattern: {}\n", pattern));
            }

            if let Some(pr_format) = &conv.pr_title_format {
                conventions.push_str(&format!("- PR title format: {}\n", pr_format));
            }

            if let Some(review) = &conv.review_requirements {
                conventions.push_str(&format!("- Review requirements: {}\n", review));
            }

            conventions.push('\n');
        }

        // Add team knowledge
        if !self.context_rules.team_knowledge.is_empty() {
            conventions.push_str("### Team Knowledge\n");
            for knowledge in &self.context_rules.team_knowledge {
                conventions.push_str(&format!("- {}\n", knowledge));
            }
            conventions.push('\n');
        }

        // Add shared memory
        if !self.shared_memory.is_empty() {
            conventions.push_str("### Shared Memory\n");
            for memory in &self.shared_memory {
                conventions.push_str(&format!("- {}\n", memory));
            }
        }

        resolved.org_conventions = Some(conventions);
    }
}

// ============================================================================
// Organization Context Loader
// ============================================================================

/// Loader for organization context from database
///
/// This loader retrieves organization-level context from the database.
/// If no database pool is provided, it operates in a no-op mode.
///
/// # Example
///
/// ```rust,ignore
/// use gateway_core::agent::context::OrganizationContextLoader;
/// use sqlx::PgPool;
/// use uuid::Uuid;
///
/// let pool = PgPool::connect("postgres://...").await?;
/// let loader = OrganizationContextLoader::new(Some(pool));
///
/// let org_id = Uuid::parse_str("...")?;
/// if let Some(ctx) = loader.load(&org_id).await? {
///     println!("Loaded org: {}", ctx.name);
/// }
/// ```
pub struct OrganizationContextLoader {
    #[cfg(feature = "database")]
    pool: Option<sqlx::Pool<sqlx::Postgres>>,

    #[cfg(not(feature = "database"))]
    _marker: std::marker::PhantomData<()>,
}

impl OrganizationContextLoader {
    /// Create a new organization context loader
    ///
    /// # Arguments
    ///
    /// * `pool` - Optional database pool. If None, the loader operates in no-op mode.
    #[cfg(feature = "database")]
    pub fn new(pool: Option<sqlx::Pool<sqlx::Postgres>>) -> Self {
        Self { pool }
    }

    /// Create a new organization context loader (no-op mode without database feature)
    #[cfg(not(feature = "database"))]
    pub fn new() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }

    /// Load organization context by organization ID
    ///
    /// # Arguments
    ///
    /// * `org_id` - The organization's unique identifier
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some(context))` if found, `Ok(None)` if not found or no database.
    #[cfg(feature = "database")]
    pub async fn load(&self, org_id: &Uuid) -> Result<Option<OrganizationContext>, Error> {
        let Some(pool) = &self.pool else {
            return Ok(None);
        };

        // Query organization from database
        let row = sqlx::query(
            r#"
            SELECT id, name, context_rules, shared_memory
            FROM organizations
            WHERE id = $1
            "#,
        )
        .bind(org_id)
        .fetch_optional(pool)
        .await?;

        match row {
            Some(r) => {
                use sqlx::Row;
                let context_rules: OrganizationRules = r
                    .try_get::<Option<serde_json::Value>, _>("context_rules")
                    .ok()
                    .flatten()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();

                let shared_memory: Vec<String> = r
                    .try_get::<Option<serde_json::Value>, _>("shared_memory")
                    .ok()
                    .flatten()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();

                Ok(Some(OrganizationContext {
                    id: r.try_get("id")?,
                    name: r.try_get("name")?,
                    context_rules,
                    shared_memory,
                }))
            }
            None => Ok(None),
        }
    }

    /// Load organization context by organization ID (no-op without database feature)
    #[cfg(not(feature = "database"))]
    pub async fn load(&self, _org_id: &Uuid) -> Result<Option<OrganizationContext>, Error> {
        Ok(None)
    }

    /// Load organization context for a user ID
    ///
    /// First retrieves the user's organization_id, then loads the organization context.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The user's unique identifier
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some(context))` if found, `Ok(None)` if user has no org or no database.
    #[cfg(feature = "database")]
    pub async fn load_for_user(
        &self,
        user_id: &Uuid,
    ) -> Result<Option<OrganizationContext>, Error> {
        let Some(pool) = &self.pool else {
            return Ok(None);
        };

        // First get user's organization_id
        let user_row = sqlx::query("SELECT organization_id FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

        match user_row {
            Some(r) => {
                use sqlx::Row;
                if let Some(org_id) = r
                    .try_get::<Option<Uuid>, _>("organization_id")
                    .ok()
                    .flatten()
                {
                    self.load(&org_id).await
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    /// Load organization context for a user ID (no-op without database feature)
    #[cfg(not(feature = "database"))]
    pub async fn load_for_user(
        &self,
        _user_id: &Uuid,
    ) -> Result<Option<OrganizationContext>, Error> {
        Ok(None)
    }

    /// Create an organization context from JSON values (for testing or manual creation)
    ///
    /// # Arguments
    ///
    /// * `id` - Organization ID
    /// * `name` - Organization name
    /// * `context_rules` - JSON value containing OrganizationRules
    /// * `shared_memory` - JSON value containing Vec<String>
    pub fn from_json(
        id: Uuid,
        name: String,
        context_rules: Option<Value>,
        shared_memory: Option<Value>,
    ) -> OrganizationContext {
        let context_rules: OrganizationRules = context_rules
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();

        let shared_memory: Vec<String> = shared_memory
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();

        OrganizationContext {
            id,
            name,
            context_rules,
            shared_memory,
        }
    }
}

#[cfg(not(feature = "database"))]
impl Default for OrganizationContextLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for OrganizationContextLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrganizationContextLoader").finish()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_organization_context_default() {
        let ctx = OrganizationContext::default();
        assert_eq!(ctx.id, Uuid::nil());
        assert!(ctx.name.is_empty());
        assert!(ctx.context_rules.team_knowledge.is_empty());
        assert!(ctx.shared_memory.is_empty());
    }

    #[test]
    fn test_org_context_applies_conventions() {
        let org = OrganizationContext {
            id: Uuid::new_v4(),
            name: "Test Org".to_string(),
            context_rules: OrganizationRules {
                code_style: Some(CodeStyleRules {
                    language: "rust".to_string(),
                    formatting: Some("rustfmt".to_string()),
                    testing: Some("cargo test".to_string()),
                    linting: None,
                    documentation: None,
                }),
                conventions: Some(ConventionRules {
                    commit_prefix: Some("[TEST]".to_string()),
                    branch_pattern: Some("feature/".to_string()),
                    pr_title_format: None,
                    review_requirements: None,
                }),
                team_knowledge: vec!["Use axum for HTTP".to_string()],
            },
            shared_memory: vec!["Previous decision: use tokio runtime".to_string()],
        };

        let mut resolved = ResolvedContext::default();
        org.apply_to(&mut resolved);

        assert!(resolved.org_conventions.is_some());
        let conv = resolved.org_conventions.unwrap();

        // Check all expected content is present
        assert!(conv.contains("Test Org"));
        assert!(conv.contains("rust"));
        assert!(conv.contains("rustfmt"));
        assert!(conv.contains("[TEST]"));
        assert!(conv.contains("feature/"));
        assert!(conv.contains("Use axum for HTTP"));
        assert!(conv.contains("use tokio runtime"));
    }

    #[test]
    fn test_org_context_layer_metadata() {
        let org = OrganizationContext::default();

        assert_eq!(org.layer_name(), "organization");
        assert_eq!(org.priority(), ContextPriority::Organization);
    }

    #[test]
    fn test_org_context_minimal_apply() {
        let org = OrganizationContext {
            id: Uuid::new_v4(),
            name: "Minimal Org".to_string(),
            context_rules: OrganizationRules::default(),
            shared_memory: vec![],
        };

        let mut resolved = ResolvedContext::default();
        org.apply_to(&mut resolved);

        assert!(resolved.org_conventions.is_some());
        let conv = resolved.org_conventions.unwrap();
        assert!(conv.contains("Minimal Org"));
    }

    #[test]
    fn test_from_json() {
        let id = Uuid::new_v4();
        let context_rules = serde_json::json!({
            "code_style": {
                "language": "python",
                "formatting": "black"
            },
            "team_knowledge": ["Use pytest for testing"]
        });
        let shared_memory = serde_json::json!(["Shared note 1", "Shared note 2"]);

        let org = OrganizationContextLoader::from_json(
            id,
            "JSON Org".to_string(),
            Some(context_rules),
            Some(shared_memory),
        );

        assert_eq!(org.id, id);
        assert_eq!(org.name, "JSON Org");
        assert!(org.context_rules.code_style.is_some());
        assert_eq!(
            org.context_rules.code_style.as_ref().unwrap().language,
            "python"
        );
        assert_eq!(org.context_rules.team_knowledge.len(), 1);
        assert_eq!(org.shared_memory.len(), 2);
    }

    #[test]
    fn test_from_json_with_none() {
        let id = Uuid::new_v4();
        let org = OrganizationContextLoader::from_json(id, "Empty Org".to_string(), None, None);

        assert_eq!(org.id, id);
        assert_eq!(org.name, "Empty Org");
        assert!(org.context_rules.code_style.is_none());
        assert!(org.context_rules.team_knowledge.is_empty());
        assert!(org.shared_memory.is_empty());
    }

    #[test]
    fn test_code_style_rules_default() {
        let rules = CodeStyleRules::default();
        assert!(rules.language.is_empty());
        assert!(rules.formatting.is_none());
        assert!(rules.testing.is_none());
    }

    #[test]
    fn test_convention_rules_default() {
        let rules = ConventionRules::default();
        assert!(rules.commit_prefix.is_none());
        assert!(rules.branch_pattern.is_none());
        assert!(rules.pr_title_format.is_none());
    }

    #[test]
    fn test_serde_round_trip() {
        let org = OrganizationContext {
            id: Uuid::new_v4(),
            name: "Serde Org".to_string(),
            context_rules: OrganizationRules {
                code_style: Some(CodeStyleRules {
                    language: "go".to_string(),
                    formatting: Some("gofmt".to_string()),
                    testing: Some("go test".to_string()),
                    linting: Some("golint".to_string()),
                    documentation: Some("godoc".to_string()),
                }),
                conventions: Some(ConventionRules {
                    commit_prefix: Some("fix:".to_string()),
                    branch_pattern: Some("bugfix/".to_string()),
                    pr_title_format: Some("[BUG] ...".to_string()),
                    review_requirements: Some("2 approvals".to_string()),
                }),
                team_knowledge: vec!["Use Go modules".to_string()],
            },
            shared_memory: vec!["Memory entry".to_string()],
        };

        // Serialize to JSON
        let json = serde_json::to_string(&org).expect("serialize");

        // Deserialize back
        let org2: OrganizationContext = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(org.id, org2.id);
        assert_eq!(org.name, org2.name);
        assert_eq!(
            org.context_rules.code_style.as_ref().unwrap().language,
            org2.context_rules.code_style.as_ref().unwrap().language
        );
        assert_eq!(org.shared_memory, org2.shared_memory);
    }

    #[cfg(not(feature = "database"))]
    #[tokio::test]
    async fn test_loader_without_database() {
        let loader = OrganizationContextLoader::new();
        let result = loader.load(&Uuid::new_v4()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());

        let result = loader.load_for_user(&Uuid::new_v4()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
