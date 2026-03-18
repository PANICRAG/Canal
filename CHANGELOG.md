# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Initial open-source release of Canal Engine
- Multi-provider LLM routing (Anthropic, OpenAI, Google, Qwen, OpenRouter, Ollama)
- 7 routing strategies (primary fallback, cascade, A/B test, task-type rules, multimodal, AI auto-select, round-robin)
- Agent execution loop with intent recognition, task planning, tool calling, and session checkpoints
- MCP gateway (client + server) with 6 pre-configured servers
- Multi-agent collaboration: Direct, Swarm, Expert modes
- Graph execution engine (LangGraph-inspired) with DAG scheduling and checkpointing
- Closed-loop learning system (experience collection, pattern mining, knowledge distillation)
- Computer Vision engine (OmniParser, UI-TARS) with workflow recording and replay
- Code execution sandbox (Python, Bash, Node.js, Go, Rust) with Docker isolation
- Semantic memory (Qdrant vector cache, plan cache, unified memory store)
- Plugin system with 10 domain-specific connector bundles
- LLM observability (Langfuse-style tracing) via devtools-core
- gRPC service definitions (Agent, LLM, Tool, Memory)
- Comprehensive documentation in English and Chinese (20 docs)
