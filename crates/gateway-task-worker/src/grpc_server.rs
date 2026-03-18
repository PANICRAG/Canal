//! gRPC server implementation for TaskWorker service

use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};
use tracing::{error, info};

use crate::file_ops::FileOperations;
use crate::git::{BranchAction, GitExecutor};
use crate::proto::*;
use crate::shell::ShellExecutor;

/// Task Worker gRPC service implementation
pub struct TaskWorkerService {
    /// Workspace directory
    workspace_dir: String,
    /// File operations handler
    file_ops: FileOperations,
    /// Shell executor
    shell: ShellExecutor,
    /// Git executor
    git: GitExecutor,
    /// Worker start time
    start_time: Instant,
    /// Active execution count
    active_executions: Arc<RwLock<u32>>,
}

impl TaskWorkerService {
    /// Create a new TaskWorkerService
    pub fn new(workspace_dir: String) -> Self {
        let file_ops = FileOperations::new(&workspace_dir);
        let shell = ShellExecutor::new(&workspace_dir);
        let git = GitExecutor::new(std::path::PathBuf::from(&workspace_dir));

        Self {
            workspace_dir,
            file_ops,
            shell,
            git,
            start_time: Instant::now(),
            active_executions: Arc::new(RwLock::new(0)),
        }
    }

    /// Increment active execution count
    async fn inc_executions(&self) {
        let mut count = self.active_executions.write().await;
        *count += 1;
    }

    /// Decrement active execution count
    async fn dec_executions(&self) {
        let mut count = self.active_executions.write().await;
        if *count > 0 {
            *count -= 1;
        }
    }
}

type ChatStream = Pin<Box<dyn Stream<Item = Result<ChatEvent, Status>> + Send>>;
type CodeStream = Pin<Box<dyn Stream<Item = Result<CodeOutput, Status>> + Send>>;
type CommandStream = Pin<Box<dyn Stream<Item = Result<CommandOutput, Status>> + Send>>;

#[tonic::async_trait]
impl task_worker_server::TaskWorker for TaskWorkerService {
    type ExecuteChatStream = ChatStream;
    type ExecuteCodeStream = CodeStream;
    type ExecuteCommandStream = CommandStream;

    /// Execute a chat request with streaming response
    async fn execute_chat(
        &self,
        request: Request<ChatRequest>,
    ) -> Result<Response<Self::ExecuteChatStream>, Status> {
        let req = request.into_inner();
        info!(
            session_id = %req.session_id,
            message_len = req.message.len(),
            "Executing chat request"
        );

        self.inc_executions().await;

        let active_executions = self.active_executions.clone();

        // For now, return a simple response
        // TODO: Integrate with full ChatHandler when Anthropic client is wired up
        let stream = async_stream::stream! {
            // Send a simple text response
            yield Ok(ChatEvent {
                event: Some(chat_event::Event::Text(TextChunk {
                    content: format!("Received message: {}", req.message),
                })),
            });

            yield Ok(ChatEvent {
                event: Some(chat_event::Event::Complete(ChatComplete {
                    message_id: uuid::Uuid::new_v4().to_string(),
                    input_tokens: 0,
                    output_tokens: 0,
                    stop_reason: "end_turn".to_string(),
                })),
            });

            // Decrement counter when stream ends
            let mut count = active_executions.write().await;
            if *count > 0 {
                *count -= 1;
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }

    /// Execute code with streaming output
    async fn execute_code(
        &self,
        request: Request<CodeRequest>,
    ) -> Result<Response<Self::ExecuteCodeStream>, Status> {
        let req = request.into_inner();
        info!(
            session_id = %req.session_id,
            language = %req.language,
            "Executing code"
        );

        self.inc_executions().await;

        let shell = self.shell.clone();
        let active_executions = self.active_executions.clone();

        let stream = async_stream::stream! {
            match shell.execute_code(&req.code, &req.language, req.timeout_seconds).await {
                Ok(events) => {
                    for event in events {
                        // Convert shell event to proto event
                        let proto_event = match event {
                            crate::shell::CodeOutputEvent::Stdout(data) => CodeOutput {
                                output: Some(code_output::Output::Stdout(data)),
                            },
                            crate::shell::CodeOutputEvent::Stderr(data) => CodeOutput {
                                output: Some(code_output::Output::Stderr(data)),
                            },
                            crate::shell::CodeOutputEvent::Complete { exit_code, execution_time_ms } => CodeOutput {
                                output: Some(code_output::Output::Complete(CodeComplete {
                                    exit_code,
                                    execution_time_ms: execution_time_ms as i64,
                                    memory_used_bytes: 0,
                                })),
                            },
                        };
                        yield Ok(proto_event);
                    }
                }
                Err(e) => {
                    error!(error = %e, "Code execution failed");
                    yield Ok(CodeOutput {
                        output: Some(code_output::Output::Error(CodeError {
                            code: "EXECUTION_ERROR".to_string(),
                            message: e.to_string(),
                        })),
                    });
                }
            }

            let mut count = active_executions.write().await;
            if *count > 0 {
                *count -= 1;
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }

    /// Execute a shell command with streaming output
    async fn execute_command(
        &self,
        request: Request<CommandRequest>,
    ) -> Result<Response<Self::ExecuteCommandStream>, Status> {
        let req = request.into_inner();
        info!(
            session_id = %req.session_id,
            command = %req.command,
            "Executing command"
        );

        self.inc_executions().await;

        let shell = self.shell.clone();
        let active_executions = self.active_executions.clone();

        let stream = async_stream::stream! {
            match shell.execute_command(&req.command, &req.args, req.timeout_seconds).await {
                Ok(events) => {
                    for event in events {
                        // Convert shell event to proto event
                        let proto_event = match event {
                            crate::shell::CommandOutputEvent::Data(data) => CommandOutput {
                                output: Some(command_output::Output::Data(data)),
                            },
                            crate::shell::CommandOutputEvent::Complete { exit_code, execution_time_ms } => CommandOutput {
                                output: Some(command_output::Output::Complete(CommandComplete {
                                    exit_code,
                                    execution_time_ms: execution_time_ms as i64,
                                })),
                            },
                        };
                        yield Ok(proto_event);
                    }
                }
                Err(e) => {
                    error!(error = %e, "Command execution failed");
                    yield Ok(CommandOutput {
                        output: Some(command_output::Output::Error(CommandError {
                            code: "COMMAND_ERROR".to_string(),
                            message: e.to_string(),
                        })),
                    });
                }
            }

            let mut count = active_executions.write().await;
            if *count > 0 {
                *count -= 1;
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }

    /// Read a file
    async fn read_file(
        &self,
        request: Request<ReadFileRequest>,
    ) -> Result<Response<FileContent>, Status> {
        let req = request.into_inner();
        info!(path = %req.path, "Reading file");

        match self
            .file_ops
            .read_file(&req.path, req.offset, req.limit)
            .await
        {
            Ok(result) => Ok(Response::new(FileContent {
                path: result.path,
                content: result.content,
                encoding: result.encoding,
                size: result.size,
                mime_type: result.mime_type,
                is_binary: result.is_binary,
                truncated: result.truncated,
            })),
            Err(e) => {
                error!(error = %e, path = %req.path, "Failed to read file");
                Err(Status::internal(e.to_string()))
            }
        }
    }

    /// Write a file
    async fn write_file(
        &self,
        request: Request<WriteFileRequest>,
    ) -> Result<Response<WriteResponse>, Status> {
        let req = request.into_inner();
        info!(path = %req.path, size = req.content.len(), "Writing file");

        match self
            .file_ops
            .write_file(&req.path, &req.content, req.create_dirs, req.overwrite)
            .await
        {
            Ok(result) => Ok(Response::new(WriteResponse {
                path: result.path,
                bytes_written: result.bytes_written,
                created: result.created,
            })),
            Err(e) => {
                error!(error = %e, path = %req.path, "Failed to write file");
                Err(Status::internal(e.to_string()))
            }
        }
    }

    /// Delete a file
    async fn delete_file(
        &self,
        request: Request<DeleteFileRequest>,
    ) -> Result<Response<DeleteResponse>, Status> {
        let req = request.into_inner();
        info!(path = %req.path, recursive = req.recursive, "Deleting file");

        match self.file_ops.delete_file(&req.path, req.recursive).await {
            Ok(result) => Ok(Response::new(DeleteResponse {
                path: result.path,
                deleted: result.deleted,
            })),
            Err(e) => {
                error!(error = %e, path = %req.path, "Failed to delete file");
                Err(Status::internal(e.to_string()))
            }
        }
    }

    /// List directory contents
    async fn list_directory(
        &self,
        request: Request<ListDirRequest>,
    ) -> Result<Response<DirectoryListing>, Status> {
        let req = request.into_inner();
        info!(path = %req.path, recursive = req.recursive, "Listing directory");

        match self
            .file_ops
            .list_directory(&req.path, req.recursive, req.max_depth)
            .await
        {
            Ok(result) => Ok(Response::new(DirectoryListing {
                path: result.path,
                entries: result
                    .entries
                    .into_iter()
                    .map(|e| FileEntry {
                        name: e.name,
                        path: e.path,
                        r#type: match e.entry_type {
                            crate::file_ops::EntryTypeResult::File => EntryType::File as i32,
                            crate::file_ops::EntryTypeResult::Directory => {
                                EntryType::Directory as i32
                            }
                            crate::file_ops::EntryTypeResult::Symlink => EntryType::Symlink as i32,
                        },
                        size: e.size,
                        modified_at: e.modified_at,
                        permissions: String::new(),
                    })
                    .collect(),
                total_count: result.total_count,
                truncated: result.truncated,
            })),
            Err(e) => {
                error!(error = %e, path = %req.path, "Failed to list directory");
                Err(Status::internal(e.to_string()))
            }
        }
    }

    /// Search files
    async fn search_files(
        &self,
        request: Request<SearchRequest>,
    ) -> Result<Response<SearchResults>, Status> {
        let req = request.into_inner();
        info!(path = %req.path, pattern = %req.pattern, "Searching files");

        match self
            .file_ops
            .search_files(&req.path, &req.pattern, req.is_regex, req.max_results)
            .await
        {
            Ok(result) => Ok(Response::new(SearchResults {
                matches: result
                    .matches
                    .into_iter()
                    .map(|m| SearchMatch {
                        file: m.file,
                        line_number: m.line_number,
                        line_content: m.line_content,
                        context_before: vec![],
                        context_after: vec![],
                    })
                    .collect(),
                total_matches: result.total_matches,
                files_searched: result.files_searched,
                truncated: result.truncated,
            })),
            Err(e) => {
                error!(error = %e, path = %req.path, "Failed to search files");
                Err(Status::internal(e.to_string()))
            }
        }
    }

    /// Clone a git repository
    async fn git_clone(
        &self,
        request: Request<GitCloneRequest>,
    ) -> Result<Response<GitCloneResponse>, Status> {
        let req = request.into_inner();
        info!(repo_url = %req.repo_url, target_path = %req.target_path, "Cloning git repository");

        let branch = if req.branch.is_empty() {
            None
        } else {
            Some(req.branch.as_str())
        };

        let depth = if req.depth > 0 { Some(req.depth) } else { None };

        match self
            .git
            .clone(&req.repo_url, &req.target_path, branch, depth)
            .await
        {
            Ok(result) => Ok(Response::new(GitCloneResponse {
                path: result.path,
                commit_hash: result.commit_hash,
                branch: result.branch,
            })),
            Err(e) => {
                error!(error = %e, repo_url = %req.repo_url, "Failed to clone repository");
                Err(Status::internal(e.to_string()))
            }
        }
    }

    /// Get git status
    async fn git_status(
        &self,
        request: Request<GitStatusRequest>,
    ) -> Result<Response<GitStatusResponse>, Status> {
        let req = request.into_inner();
        info!(path = %req.path, "Getting git status");

        match self.git.status(&req.path).await {
            Ok(result) => Ok(Response::new(GitStatusResponse {
                branch: result.branch,
                commit_hash: result.commit_hash,
                files: result
                    .files
                    .into_iter()
                    .map(|f| GitFileStatus {
                        path: f.path,
                        status: f.status,
                        staged_status: f.staged_status,
                    })
                    .collect(),
                is_clean: result.is_clean,
                ahead: result.ahead,
                behind: result.behind,
            })),
            Err(e) => {
                error!(error = %e, path = %req.path, "Failed to get git status");
                Err(Status::internal(e.to_string()))
            }
        }
    }

    /// Create a git commit
    async fn git_commit(
        &self,
        request: Request<GitCommitRequest>,
    ) -> Result<Response<GitCommitResponse>, Status> {
        let req = request.into_inner();
        info!(path = %req.path, message = %req.message, "Creating git commit");

        let author_name = if req.author_name.is_empty() {
            None
        } else {
            Some(req.author_name.as_str())
        };

        let author_email = if req.author_email.is_empty() {
            None
        } else {
            Some(req.author_email.as_str())
        };

        match self
            .git
            .commit(
                &req.path,
                &req.message,
                &req.files,
                author_name,
                author_email,
            )
            .await
        {
            Ok(result) => Ok(Response::new(GitCommitResponse {
                commit_hash: result.commit_hash,
                message: result.message,
                files_changed: result.files_changed,
            })),
            Err(e) => {
                error!(error = %e, path = %req.path, "Failed to create git commit");
                Err(Status::internal(e.to_string()))
            }
        }
    }

    /// Get git diff
    async fn git_diff(
        &self,
        request: Request<GitDiffRequest>,
    ) -> Result<Response<GitDiffResponse>, Status> {
        let req = request.into_inner();
        info!(path = %req.path, "Getting git diff");

        let base_ref = if req.base_ref.is_empty() {
            None
        } else {
            Some(req.base_ref.as_str())
        };

        let target_ref = if req.target_ref.is_empty() {
            None
        } else {
            Some(req.target_ref.as_str())
        };

        match self
            .git
            .diff(&req.path, base_ref, target_ref, &req.files)
            .await
        {
            Ok(result) => Ok(Response::new(GitDiffResponse {
                diffs: result
                    .diffs
                    .into_iter()
                    .map(|d| GitFileDiff {
                        path: d.path,
                        status: d.status,
                        additions: d.additions,
                        deletions: d.deletions,
                        patch: d.patch,
                    })
                    .collect(),
                total_additions: result.total_additions,
                total_deletions: result.total_deletions,
            })),
            Err(e) => {
                error!(error = %e, path = %req.path, "Failed to get git diff");
                Err(Status::internal(e.to_string()))
            }
        }
    }

    /// Git branch operations
    async fn git_branch(
        &self,
        request: Request<GitBranchRequest>,
    ) -> Result<Response<GitBranchResponse>, Status> {
        let req = request.into_inner();
        info!(path = %req.path, "Git branch operation");

        let action = match req.operation {
            Some(git_branch_request::Operation::Checkout(name)) => BranchAction::Checkout(name),
            Some(git_branch_request::Operation::Create(name)) => BranchAction::Create(name),
            Some(git_branch_request::Operation::Delete(name)) => BranchAction::Delete(name),
            None => BranchAction::List,
        };

        match self.git.branch(&req.path, action).await {
            Ok(result) => Ok(Response::new(GitBranchResponse {
                current_branch: result.current_branch,
                branches: result.branches,
            })),
            Err(e) => {
                error!(error = %e, path = %req.path, "Failed to perform git branch operation");
                Err(Status::internal(e.to_string()))
            }
        }
    }

    /// Heartbeat check
    async fn heartbeat(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<HeartbeatResponse>, Status> {
        Ok(Response::new(HeartbeatResponse {
            alive: true,
            uptime_seconds: self.start_time.elapsed().as_secs() as i64,
            timestamp: chrono::Utc::now().timestamp(),
        }))
    }

    /// Get worker status
    async fn get_status(&self, _request: Request<Empty>) -> Result<Response<WorkerStatus>, Status> {
        let active = *self.active_executions.read().await;

        Ok(Response::new(WorkerStatus {
            worker_id: std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string()),
            session_id: "".to_string(),
            state: if active > 0 {
                WorkerState::Busy as i32
            } else {
                WorkerState::Ready as i32
            },
            // R8-M9: Report None instead of fake zeros — consumers can distinguish
            // "no data" from "actually zero"
            resources: None,
            uptime_seconds: self.start_time.elapsed().as_secs() as i64,
            active_executions: active as i32,
            available_languages: vec![
                "python".to_string(),
                "bash".to_string(),
                "javascript".to_string(),
            ],
        }))
    }

    /// Shutdown the worker
    async fn shutdown(
        &self,
        request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownResponse>, Status> {
        let req = request.into_inner();
        info!(graceful = req.graceful, "Shutdown requested");

        // R8-M11: Log warning that shutdown is not fully implemented
        tracing::warn!("Shutdown handler received request but process-level shutdown is not wired. The worker will continue running until externally terminated.");
        Ok(Response::new(ShutdownResponse {
            accepted: true,
            message: "Shutdown acknowledged but not yet implemented — worker continues running"
                .to_string(),
        }))
    }
}
