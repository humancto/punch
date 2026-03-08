//! # punch-runtime
//!
//! The agent execution engine for the Punch Agent Combat System.
//!
//! This crate contains the core fighter loop, LLM driver abstraction,
//! tool execution engine, MCP client, and loop guard / circuit breaker.
//!
//! ## Terminology
//!
//! - **Fighter**: An AI agent (conversational or task-oriented)
//! - **Gorilla**: An autonomous agent that runs without user prompts
//! - **Bout**: A session / conversation
//! - **Move**: A tool invocation

pub mod driver;
pub mod fighter_loop;
pub mod guard;
pub mod mcp;
pub mod tool_executor;

pub use driver::{
    AnthropicDriver, CompletionRequest, CompletionResponse, LlmDriver, OpenAiCompatibleDriver,
    StopReason, TokenUsage, create_driver,
};
pub use fighter_loop::{FighterLoopParams, FighterLoopResult, run_fighter_loop};
pub use guard::{LoopGuard, LoopGuardVerdict};
pub use mcp::McpClient;
pub use tool_executor::{ToolExecutionContext, execute_tool};
