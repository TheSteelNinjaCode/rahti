mod project;

use rmcp::{
    RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{
        ErrorData, ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
        ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
        ServerInfo,
    },
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::Value;

pub use project::Project;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct DiagnosticsRequest {
    #[schemars(description = "Number of newest JSONL entries to return (1-200; default 50)")]
    limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct RahtiMcp {
    project: Project,
    #[allow(dead_code)] // read by the `tool_handler` expansion
    tool_router: ToolRouter<Self>,
}

impl RahtiMcp {
    pub fn new(project: Project) -> Self {
        Self {
            project,
            tool_router: Self::tool_router(),
        }
    }

    fn tool_result(result: Result<Value, String>) -> Result<Json<Value>, ErrorData> {
        result
            .map(Json)
            .map_err(|message| ErrorData::internal_error(message, None))
    }
}

#[tool_router]
impl RahtiMcp {
    #[tool(description = "Summarize the Rahti project and its authentication boundaries")]
    fn project_summary(&self) -> Result<Json<Value>, ErrorData> {
        Self::tool_result(self.project.summary())
    }

    #[tool(description = "List generated page and API routes")]
    fn list_routes(&self) -> Result<Json<Value>, ErrorData> {
        Self::tool_result(self.project.routes())
    }

    #[tool(description = "List application components and their component-owned RPCs")]
    fn list_components(&self) -> Result<Json<Value>, ErrorData> {
        Self::tool_result(self.project.components())
    }

    #[tool(description = "List page and component RPCs, including whether each requires a session")]
    fn list_rpcs(&self) -> Result<Json<Value>, ErrorData> {
        Self::tool_result(self.project.rpcs())
    }

    #[tool(
        description = "List WebSocket endpoints, including whether each handshake requires a session"
    )]
    fn list_sockets(&self) -> Result<Json<Value>, ErrorData> {
        Self::tool_result(self.project.sockets())
    }

    #[tool(description = "List configured SeaORM backend, model files, and migration files")]
    fn list_database_artifacts(&self) -> Result<Json<Value>, ErrorData> {
        Self::tool_result(self.project.database_artifacts())
    }

    #[tool(description = "Validate generated manifest references without changing the project")]
    fn validate_project(&self) -> Result<Json<Value>, ErrorData> {
        Self::tool_result(self.project.validate())
    }

    #[tool(description = "Read the newest bounded entries from .rahti/dev.log")]
    fn recent_diagnostics(
        &self,
        Parameters(request): Parameters<DiagnosticsRequest>,
    ) -> Result<Json<Value>, ErrorData> {
        Self::tool_result(self.project.recent_diagnostics(request.limit))
    }
}

#[tool_handler]
impl ServerHandler for RahtiMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_instructions(
            "Read-only Rahti project context. This stdio server never reads .env, never mutates the project, and does not act as a browser-authenticated application user.",
        )
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, ErrorData>> + Send + '_ {
        let conventions = self.project.convention_resources();
        async move {
            let mut resources = vec![
                Resource::new("rahti://project/config", "Rahti project configuration")
                    .with_mime_type("application/json"),
                Resource::new(
                    "rahti://project/manifest",
                    "Generated Rahti project manifest",
                )
                .with_mime_type("application/json"),
            ];
            resources.extend(
                conventions
                    .map_err(|message| ErrorData::internal_error(message, None))?
                    .into_iter()
                    .map(|(uri, name, _)| {
                        Resource::new(uri, format!("Rahti convention: {name}"))
                            .with_mime_type("text/markdown")
                    }),
            );
            Ok(ListResourcesResult::with_all_items(resources))
        }
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let contents = self
            .project
            .read_resource(&request.uri)
            .map_err(|message| ErrorData::resource_not_found(message, None))?;
        Ok(ReadResourceResult::new(vec![ResourceContents::text(contents, request.uri)]).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::TestProject;
    use rmcp::{ServiceExt, model::ReadResourceRequestParams};

    #[test]
    fn advertises_read_only_tools_and_resources() {
        let fixture = TestProject::new();
        let server = RahtiMcp::new(fixture.project());
        let info = server.get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_some());
        assert!(info.instructions.unwrap().contains("never reads .env"));
    }

    #[tokio::test]
    async fn serves_tools_and_resources_over_the_mcp_protocol() {
        let fixture = TestProject::new();
        let server = RahtiMcp::new(fixture.project());
        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(async move {
            server
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let client = ().serve(client_transport).await.unwrap();

        let tools = client.list_all_tools().await.unwrap();
        assert!(tools.iter().any(|tool| tool.name == "list_rpcs"));
        assert!(tools.iter().any(|tool| tool.name == "recent_diagnostics"));

        let resources = client.list_all_resources().await.unwrap();
        assert!(
            resources
                .iter()
                .any(|resource| resource.uri == "rahti://project/manifest")
        );
        let manifest = client
            .read_resource_once(ReadResourceRequestParams::new("rahti://project/manifest"))
            .await
            .unwrap();
        match manifest {
            rmcp::model::ReadResourceResponse::Complete(result) => {
                assert!(!result.contents.is_empty());
            }
            _ => panic!("manifest resource unexpectedly requested client input"),
        }

        client.cancel().await.unwrap();
        server_task.await.unwrap();
    }
}
