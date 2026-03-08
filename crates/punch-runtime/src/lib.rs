//! # punch-runtime
//!
//! The agent execution engine for the Punch Agent Combat System.
//!
//! This crate contains the core fighter loop, LLM driver abstraction,
//! tool execution engine, MCP client, loop guard / circuit breaker,
//! context budget management, and session repair.
//!
//! ## Terminology
//!
//! - **Fighter**: An AI agent (conversational or task-oriented)
//! - **Gorilla**: An autonomous agent that runs without user prompts
//! - **Bout**: A session / conversation
//! - **Move**: A tool invocation

pub mod context_budget;
pub mod driver;
pub mod fighter_loop;
pub mod guard;
pub mod mcp;
pub mod session_repair;
pub mod tool_executor;
pub mod tools;

pub use context_budget::{ContextBudget, TrimAction};
pub use driver::{
    AnthropicDriver, CompletionRequest, CompletionResponse, LlmDriver, OpenAiCompatibleDriver,
    StopReason, TokenUsage, create_driver,
};
pub use fighter_loop::{FighterLoopParams, FighterLoopResult, run_fighter_loop};
pub use guard::{GuardConfig, GuardVerdict, LoopGuard, LoopGuardVerdict};
pub use mcp::McpClient;
pub use session_repair::{RepairStats, repair_session};
pub use tool_executor::{ToolExecutionContext, execute_tool};
pub use tools::{all_tools, tools_for_capabilities};
