//! `yrem mcp`: stdio MCP server (rmcp). One flat tool per CLI leaf command.

mod schema;

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, Implementation, ServerCapabilities, ServerConfig,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use serde_json::Value;

pub use schema::{compact, compact_schema};

use crate::error::Result;
use crate::ops::{
    App, ConflictListArgs, ConflictResolveArgs, CorrectArgs, ForgetArgs, GraphEdgesArgs,
    GraphRelateArgs, MemoryCurrentArgs, MemoryGetArgs, RecallArgs, RememberArgs,
    SessionCaptureArgs, SessionDigestArgs, ThinkArgs,
};

/// MCP server instructions (~145 tokens).
pub const INSTRUCTIONS: &str = include_str!("instructions.md");

type ToolResult = std::result::Result<CallToolResult, ErrorData>;

/// The MCP server: tools delegate to [`App`].
#[derive(Debug, Clone)]
pub struct Server {
    app: Arc<App>,
}

impl Server {
    #[must_use]
    pub fn new(app: App) -> Self {
        Self { app: Arc::new(app) }
    }

    /// Serves MCP on stdin / stdout until the client disconnects.
    ///
    /// # Errors
    /// Transport or protocol initialisation failures.
    pub async fn serve_stdio(self) -> anyhow::Result<()> {
        self.serve(rmcp::transport::stdio())
            .await?
            .waiting()
            .await?;
        Ok(())
    }
}

/// Runs `f` until it finishes or the request is cancelled. Cancelling drops the
/// future, which aborts the in-flight HTTP request.
async fn cancellable(
    ctx: &RequestContext<RoleServer>,
    f: impl AsyncFnOnce() -> Result<Value>,
) -> ToolResult {
    tokio::select! {
        () = ctx.ct.cancelled() => Err(ErrorData::internal_error("request cancelled", None)),
        r = f() => Ok(to_result(r)),
    }
}

/// Success → compact JSON text; library error → `isError` result with actionable text.
#[must_use]
pub fn to_result(r: Result<Value>) -> CallToolResult {
    match r {
        Ok(v) => CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string(&v).unwrap_or_default(),
        )]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
    }
}

#[tool_router]
impl Server {
    #[tool(
        description = "Store a durable fact. Specific, self-contained text.",
        input_schema = compact_schema::<RememberArgs>(),
        annotations(read_only_hint = false, destructive_hint = false, open_world_hint = false)
    )]
    async fn remember(
        &self,
        Parameters(a): Parameters<RememberArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        cancellable(&ctx, async || self.app.remember(a).await).await
    }

    #[tool(
        description = "Semantic search over memories. Returns ranked hits with rids.",
        input_schema = compact_schema::<RecallArgs>(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn recall(
        &self,
        Parameters(a): Parameters<RecallArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        cancellable(&ctx, async || self.app.recall(a).await).await
    }

    #[tool(
        description = "Delete a memory by rid.",
        input_schema = compact_schema::<ForgetArgs>(),
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn forget(
        &self,
        Parameters(a): Parameters<ForgetArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        cancellable(&ctx, async || self.app.forget(a).await).await
    }

    #[tool(
        description = "Replace a stored fact that changed. New record links the old rid; its created_at is now.",
        input_schema = compact_schema::<CorrectArgs>(),
        annotations(read_only_hint = false, destructive_hint = true, open_world_hint = false)
    )]
    async fn correct(
        &self,
        Parameters(a): Parameters<CorrectArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        cancellable(&ctx, async || self.app.correct(a).await).await
    }

    #[tool(
        description = "Consolidate memories and detect conflicts.",
        input_schema = compact_schema::<ThinkArgs>(),
        annotations(read_only_hint = false, destructive_hint = false, open_world_hint = false)
    )]
    async fn think(
        &self,
        Parameters(a): Parameters<ThinkArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        cancellable(&ctx, async || self.app.think(a).await).await
    }

    #[tool(
        description = "Link two entities with a typed relation.",
        input_schema = compact_schema::<GraphRelateArgs>(),
        annotations(read_only_hint = false, destructive_hint = false, open_world_hint = false)
    )]
    async fn graph_relate(
        &self,
        Parameters(a): Parameters<GraphRelateArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        cancellable(&ctx, async || self.app.graph_relate(a).await).await
    }

    #[tool(
        description = "List an entity's relations.",
        input_schema = compact_schema::<GraphEdgesArgs>(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn graph_edges(
        &self,
        Parameters(a): Parameters<GraphEdgesArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        cancellable(&ctx, async || self.app.graph_edges(a).await).await
    }

    #[tool(
        description = "List open conflicts between stored facts.",
        input_schema = compact_schema::<ConflictListArgs>(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn conflict_list(
        &self,
        Parameters(a): Parameters<ConflictListArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        cancellable(&ctx, async || self.app.conflict_list(a).await).await
    }

    #[tool(
        description = "Resolve a conflict.",
        input_schema = compact_schema::<ConflictResolveArgs>(),
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn conflict_resolve(
        &self,
        Parameters(a): Parameters<ConflictResolveArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        cancellable(&ctx, async || self.app.conflict_resolve(a).await).await
    }

    #[tool(
        description = "Session-start briefing: open decisions, conflicts, stale facts.",
        input_schema = compact_schema::<SessionDigestArgs>(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn session_digest(
        &self,
        Parameters(a): Parameters<SessionDigestArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        cancellable(&ctx, async || self.app.session_digest(a).await).await
    }

    #[tool(
        description = "Split a session summary into separate stored facts.",
        input_schema = compact_schema::<SessionCaptureArgs>(),
        annotations(read_only_hint = false, destructive_hint = false, open_world_hint = false)
    )]
    async fn session_capture(
        &self,
        Parameters(a): Parameters<SessionCaptureArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        cancellable(&ctx, async || self.app.session_capture(a).await).await
    }

    #[tool(
        description = "Fetch one memory by rid.",
        input_schema = compact_schema::<MemoryGetArgs>(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn memory_get(
        &self,
        Parameters(a): Parameters<MemoryGetArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        cancellable(&ctx, async || self.app.memory_get(a).await).await
    }

    #[tool(
        description = "Latest value of a fact that changes over time.",
        input_schema = compact_schema::<MemoryCurrentArgs>(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn memory_current(
        &self,
        Parameters(a): Parameters<MemoryCurrentArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        cancellable(&ctx, async || self.app.memory_current(a).await).await
    }
}

#[tool_handler]
#[expect(clippy::unused_async_trait_impl, reason = "generated by rmcp macros")]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("yrem", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS.to_owned())
    }
}
