//! AI Gateway CLI
//!
//! Command-line interface for managing and interacting with the AI Gateway.

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;

mod commands;

#[derive(Parser)]
#[command(name = "gateway")]
#[command(author, version, about = "AI Gateway CLI", long_about = None)]
struct Cli {
    /// Gateway API URL
    #[arg(
        short,
        long,
        env = "GATEWAY_URL",
        default_value = "http://localhost:4000"
    )]
    url: String,

    /// API key for authentication
    #[arg(short = 'k', long, env = "GATEWAY_API_KEY")]
    api_key: Option<String>,

    /// Enable verbose output
    #[arg(short, long)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Chat with an LLM through the gateway
    Chat {
        /// The message to send
        message: String,

        /// Model to use (optional)
        #[arg(short, long)]
        model: Option<String>,
        // R9-H10: --stream flag removed — was accepted but never implemented.
        // Streaming support requires SSE parsing which is not yet available in the CLI.
    },

    /// List available tools from MCP servers
    Tools {
        #[command(subcommand)]
        action: ToolsAction,
    },

    /// Manage workflows
    Workflows {
        #[command(subcommand)]
        action: WorkflowsAction,
    },

    /// Manage MCP server connections
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },

    /// Check gateway health
    Health,

    /// Show gateway configuration
    Config,
}

#[derive(Subcommand)]
enum ToolsAction {
    /// List all available tools
    List,
    /// Call a specific tool
    Call {
        /// Tool name in format: namespace.tool
        name: String,
        /// JSON input for the tool
        #[arg(short, long)]
        input: Option<String>,
    },
}

#[derive(Subcommand)]
enum WorkflowsAction {
    /// List all workflows
    List,
    /// Get workflow details
    Get {
        /// Workflow ID
        id: String,
    },
    /// Execute a workflow
    Execute {
        /// Workflow ID
        id: String,
        /// JSON input for the workflow
        #[arg(short, long)]
        input: Option<String>,
    },
}

#[derive(Subcommand)]
enum McpAction {
    /// List connected MCP servers
    List,
    /// Get MCP server status
    Status {
        /// Server ID
        id: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    // Initialize logging
    if cli.verbose {
        tracing_subscriber::fmt()
            .with_env_filter("gateway_cli=debug")
            .init();
    }

    // Create HTTP client
    let client = commands::GatewayClient::new(&cli.url, cli.api_key.as_deref())?;

    // Execute command
    match cli.command {
        Commands::Chat { message, model } => {
            commands::chat(&client, &message, model.as_deref()).await?;
        }
        Commands::Tools { action } => match action {
            ToolsAction::List => {
                commands::tools_list(&client).await?;
            }
            ToolsAction::Call { name, input } => {
                commands::tools_call(&client, &name, input.as_deref()).await?;
            }
        },
        Commands::Workflows { action } => match action {
            WorkflowsAction::List => {
                commands::workflows_list(&client).await?;
            }
            WorkflowsAction::Get { id } => {
                commands::workflows_get(&client, &id).await?;
            }
            WorkflowsAction::Execute { id, input } => {
                commands::workflows_execute(&client, &id, input.as_deref()).await?;
            }
        },
        Commands::Mcp { action } => match action {
            McpAction::List => {
                commands::mcp_list(&client).await?;
            }
            McpAction::Status { id } => {
                commands::mcp_status(&client, &id).await?;
            }
        },
        Commands::Health => {
            commands::health(&client).await?;
        }
        Commands::Config => {
            println!("{}", "Gateway Configuration".bold());
            println!("  URL: {}", cli.url.cyan());
            println!(
                "  API Key: {}",
                cli.api_key
                    .as_ref()
                    // R9-M11: Show only first 4 chars to reduce credential exposure
                    .map(|k| format!("{}...", &k[..4.min(k.len())]))
                    .unwrap_or_else(|| "Not set".to_string())
                    .yellow()
            );
        }
    }

    Ok(())
}
